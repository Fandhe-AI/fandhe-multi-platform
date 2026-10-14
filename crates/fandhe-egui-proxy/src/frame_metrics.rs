//! 固定 60Hz 環境でのフレームレート計測ワークロード（TASK-5.3 / REQ-5、Issue #238）。
//!
//! PoC-4（`docs/spec/03-poc/performance/README.md`）の macOS プロキシ実測は
//! ProMotion（可変リフレッシュレート）のノイズを含み「条件付き達成・参考値」に
//! 留まったため、固定 60Hz パネル実機（iPhone 15 / Pixel 10a、`min_refresh_rate`
//! 固定後）で PoC-4 と同じプロトコル（300 項目リスト・warmup 破棄・p95 パーセンタイル・
//! 16.6ms 超過率）を再計測するための専用アプリをここに実装する。
//!
//! **feature ゲート契約**: 本モジュールは非 default cargo feature `frame-metrics`
//! （`Cargo.toml` 参照）が有効な場合のみコンパイル対象になる加法的（additive）な
//! 追加であり、通常ビルド（feature 無効）の挙動・依存関係を一切変更しない
//! （`.claude/rules/coding-rust.md` の feature 設計方針）。呼び出し元は feature 有効時の
//! `main.rs`（デスクトップ相当）・`ios.rs`（`run` 内の feature 分岐）・`android.rs`
//! （`android_main` の feature 分岐）の 3 箇所のみで、いずれも計測専用ビルドへ
//! エントリそのものを差し替える（既存の `ProxyApp` 経路とは共存しない別ビルド）。
//! wasm32 は対象外（Web の計測は既存の `web/index.html` `?measure=1` 計装、TASK-4.5 を使う）。
//!
//! **計測プロトコル**（PoC-4 準拠）: [`fandhe_core::Screen::virtual_list`] で 300 項目の
//! `Node::VirtualList` 画面を構築し、`render_screen`（`lib.rs`）の実描画パス
//! （`render_virtual_list` の可視範囲限定描画）を通す。`ctx.request_repaint()` で
//! 毎フレーム再描画を要求し、vsync ペーシング下で 60Hz 律速の連続再描画を駆動する。
//! 可視項目のテキストは [`FrameMetricsState::tick`]（`Cell` による内部可変性。
//! 状態アクセスは `fn(&S) -> T` 関数ポインタ限定という DSL の制約（REQ-13）の範囲内で
//! 毎フレーム変化させるための設計）に応じて切り替わり、実レイアウトの再計算を伴う
//! 「脈動アニメーション」相当の負荷を生む（`render_virtual_list` 内部の
//! `egui::ScrollArea` へ外部からスクロール位置を注入する API が無いため、
//! 自動スクロールではなくこの脈動負荷を計測ワークロードとして採用する。
//! 計画書 §8 リスク (2) のフォールバック）。
//!
//! warmup 120 フレームを破棄した後、600 フレーム（約 10 秒 @ 60Hz）のフレーム間隔を
//! 収集し、p50 / p95 / max / 16.6ms 超過率（フレーム落ち率）を算出する
//! （[`summarize`]）。判定は「p95 が 16.6ms 以内かつ超過率 5% 未満」（REQ-5 受け入れ基準）。
//!
//! 結果出力は 3 経路（[`FrameMetricsApp::on_measurement_complete`]）:
//! (a) 画面内オーバーレイ（`egui::Window`。実機でのスクリーンショット記録用）、
//! (b) 構造化ログ 1 行 `FANDHE_FRAME_METRICS=<json>` を stderr へ（デスクトップ/
//! シミュレータでの `--console` 捕捉用）、(c) Android のみ `result_path`
//! （`AndroidApp::internal_data_path()` 配下、`android.rs` の feature 分岐が設定）へ
//! JSON ファイル書き出し。

use std::cell::Cell;

use fandhe_core::{Node, Screen, StateStore};

/// 計測ワークロードの総項目数（PoC-4 のプロトコルと同じ 300 項目基準、REQ-5）。
const ITEM_COUNT: usize = 300;

/// 1 項目あたりの固定高さ（px）。`render_virtual_list`（`lib.rs`）の
/// 「1 項目 = `item_extent` px」の幾何モデルにそのまま渡す値。
const ITEM_EXTENT: f32 = 24.0;

/// warmup として破棄するフレーム数。初期化・レイアウト確定直後の外れ値を
/// 除外する（PoC-4 と同じ考え方）。
const WARMUP_FRAMES: u32 = 120;

/// 収集するサンプルフレーム数（約 10 秒 @ 60Hz）。
const SAMPLE_FRAMES: usize = 600;

/// 60Hz の 1 フレーム予算（ミリ秒）。REQ-5 受け入れ基準「95 パーセンタイルで
/// 16.6ms 以内」の判定基準値そのもの。
const FRAME_BUDGET_MS: f64 = 1000.0 / 60.0;

/// 計測ワークロード画面が保持する最小限の状態。
///
/// `Node::VirtualList::item` は `fn(&S, usize) -> Node<S>`（`ReadIndexedFn`、
/// `fandhe-core` の SSOT 型）という関数ポインタ限定の契約（REQ-13、クロージャの
/// 環境キャプチャを型レベルで拒否する AI 型安全 DSL の方針）のため、毎フレーム
/// 変化する「脈動」の位相をこの関数ポインタ経由（`&S` の共有参照のみ）で読む必要がある。
/// `Cell` による内部可変性を使い、`StateStore::dispatch`（`Event` 経由の書き込み）を
/// 経由せずに `update` から直接進められるようにする（本計測ワークロードは
/// ユーザー操作を模した `Event` を持たないための最小限の逸脱。`&mut S` を外部へ
/// 公開しない設計方針そのものは崩さない）。
#[derive(Default)]
struct FrameMetricsState {
    /// フレームごとに 1 ずつ進む位相カウンタ。[`frame_metrics_item`] が
    /// 可視項目の表示切り替えに使う。
    tick: Cell<u64>,
}

/// 状態からは常に固定の [`ITEM_COUNT`] を返す（`ReadFn<S, usize>` 契約）。
fn frame_metrics_len(_state: &FrameMetricsState) -> usize {
    ITEM_COUNT
}

/// index 番目の項目ノードを構築する（`ReadIndexedFn<S, Node<S>>` 契約）。
///
/// `state.tick` の位相に応じて項目内の記号を切り替え、可視項目のテキスト内容が
/// 毎フレーム変わる負荷（脈動アニメーション相当、モジュール doc 参照）を生む。
fn frame_metrics_item(state: &FrameMetricsState, index: usize) -> Node<FrameMetricsState> {
    let phase = (state.tick.get() % 20) as usize;
    let marker = if index % 20 == phase { "◆" } else { "・" };
    Node::Text(format!("{marker} item {index:03}"))
}

/// 計測ワークロード用の 1 画面を構築する（300 項目 `VirtualList`）。
fn frame_metrics_screen() -> Screen<FrameMetricsState> {
    Screen::new("frame-metrics").virtual_list(frame_metrics_len, frame_metrics_item, ITEM_EXTENT)
}

/// p50 / p95 / max / 超過率の算出結果。stderr 出力・Android ファイル出力・
/// 画面オーバーレイの 3 経路で共有する（[`FrameMetricsSummary::to_json`]）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FrameMetricsSummary {
    /// 収集できたサンプルフレーム数（warmup 除外後）。
    pub sample_count: usize,
    /// フレーム間隔の 50 パーセンタイル（ミリ秒）。
    pub p50_ms: f64,
    /// フレーム間隔の 95 パーセンタイル（ミリ秒）。REQ-5 受け入れ基準の判定対象。
    pub p95_ms: f64,
    /// フレーム間隔の最大値（ミリ秒）。
    pub max_ms: f64,
    /// [`FRAME_BUDGET_MS`]（16.6ms）を超えたサンプルの比率（フレーム落ち率）。
    /// REQ-5 受け入れ基準「フレーム落ち 5% 未満」の判定対象。
    pub over_budget_ratio: f64,
}

impl FrameMetricsSummary {
    /// 3 出力経路（オーバーレイ・stderr・Android ファイル）で共有する JSON 表現を返す。
    ///
    /// `serde_json` 等の依存を追加せず（feature 追加時の依存追加なしという設計方針、
    /// モジュール doc 参照）、数値のみの単純な構造のため手組みで整形する。
    pub(crate) fn to_json(&self) -> String {
        format!(
            "{{\"sample_count\":{},\"p50_ms\":{:.3},\"p95_ms\":{:.3},\"max_ms\":{:.3},\"over_budget_ratio\":{:.4}}}",
            self.sample_count, self.p50_ms, self.p95_ms, self.max_ms, self.over_budget_ratio
        )
    }
}

/// 昇順ソート済みスライス `sorted_ms` からパーセンタイル `p`（0.0〜100.0）を線形補間で求める。
///
/// `unwrap()` / `expect()` を使わない（`.claude/rules/coding-rust.md` のエラー
/// ハンドリング方針）。空スライスは `0.0` を返す安全側フォールバック。
fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let last_index = sorted_ms.len().saturating_sub(1) as f64;
    let rank = (p / 100.0) * last_index;
    let lower = rank.floor().max(0.0) as usize;
    let upper = rank.ceil().max(0.0) as usize;
    let lower = lower.min(sorted_ms.len() - 1);
    let upper = upper.min(sorted_ms.len() - 1);
    if lower == upper {
        sorted_ms[lower]
    } else {
        let fraction = rank - lower as f64;
        sorted_ms[lower] + (sorted_ms[upper] - sorted_ms[lower]) * fraction
    }
}

/// 収集したフレーム間隔 `samples_ms` から [`FrameMetricsSummary`] を算出する
/// （PoC-4 と同じ p50/p95/max/超過率の算出プロトコル）。
fn summarize(samples_ms: &[f64], budget_ms: f64) -> FrameMetricsSummary {
    let mut sorted = samples_ms.to_vec();
    sorted.sort_by(f64::total_cmp);

    let sample_count = sorted.len();
    let p50_ms = percentile(&sorted, 50.0);
    let p95_ms = percentile(&sorted, 95.0);
    let max_ms = sorted.iter().copied().last().unwrap_or(0.0);
    let over_count = sorted.iter().filter(|&&v| v > budget_ms).count();
    let over_budget_ratio = if sample_count == 0 {
        0.0
    } else {
        over_count as f64 / sample_count as f64
    };

    FrameMetricsSummary {
        sample_count,
        p50_ms,
        p95_ms,
        max_ms,
        over_budget_ratio,
    }
}

/// warmup 破棄・サンプル収集・パーセンタイル算出までを進める状態機械。
///
/// `update` から毎フレーム [`Self::record_frame`] を呼ぶだけで、warmup 経過後の
/// 600 フレーム収集・算出済み判定（[`Self::summary`]）までを内部で完結させる。
struct FrameMetricsCollector {
    warmup_remaining: u32,
    samples_ms: Vec<f64>,
    last_instant: Option<std::time::Instant>,
    summary: Option<FrameMetricsSummary>,
}

impl FrameMetricsCollector {
    fn new() -> Self {
        Self {
            warmup_remaining: WARMUP_FRAMES,
            samples_ms: Vec::with_capacity(SAMPLE_FRAMES),
            last_instant: None,
            summary: None,
        }
    }

    /// 1 フレーム分の経過を記録する。算出済み（[`Self::summary`] が `Some`）以降は
    /// 何もしない（収集済みサンプルを上書きしない）。
    fn record_frame(&mut self, now: std::time::Instant) {
        if self.summary.is_some() {
            return;
        }
        if let Some(prev) = self.last_instant {
            let elapsed_ms = now.duration_since(prev).as_secs_f64() * 1000.0;
            if self.warmup_remaining > 0 {
                self.warmup_remaining -= 1;
            } else if self.samples_ms.len() < SAMPLE_FRAMES {
                self.samples_ms.push(elapsed_ms);
            }
        }
        self.last_instant = Some(now);

        if self.warmup_remaining == 0 && self.samples_ms.len() >= SAMPLE_FRAMES {
            self.summary = Some(summarize(&self.samples_ms, FRAME_BUDGET_MS));
        }
    }

    fn summary(&self) -> Option<&FrameMetricsSummary> {
        self.summary.as_ref()
    }
}

/// 計測ワークロードを描画しつつフレーム間隔を収集する `eframe::App` 実装。
///
/// `main.rs` / `ios.rs` / `android.rs` の feature 分岐からのみ構築される
/// （モジュール doc の「feature ゲート契約」参照）。
pub struct FrameMetricsApp {
    screen: Screen<FrameMetricsState>,
    store: StateStore<FrameMetricsState>,
    collector: FrameMetricsCollector,
    /// 計測完了時に JSON を書き出す先（Android のみ `Some`。
    /// `AndroidApp::internal_data_path()` 配下、`run_android` 参照）。
    result_path: Option<std::path::PathBuf>,
}

impl FrameMetricsApp {
    /// 計測アプリを構築する。`result_path` は Android のみ結果 JSON の書き出し先を指定する
    /// （デスクトップ/iOS は `None`、オーバーレイと stderr のみで結果を提示する）。
    pub fn new(cc: &eframe::CreationContext<'_>, result_path: Option<std::path::PathBuf>) -> Self {
        crate::font::install_cjk_font(&cc.egui_ctx);
        Self {
            screen: frame_metrics_screen(),
            store: StateStore::new(FrameMetricsState::default()),
            collector: FrameMetricsCollector::new(),
            result_path,
        }
    }

    /// 計測完了の瞬間に 1 度だけ呼ばれ、stderr ログ・（Android のみ）結果ファイルへ
    /// 出力する（モジュール doc の出力経路 (b)(c)）。オーバーレイ表示 (a) は
    /// `update` 側が毎フレーム描画する。
    fn on_measurement_complete(&self, summary: &FrameMetricsSummary) {
        eprintln!("FANDHE_FRAME_METRICS={}", summary.to_json());

        if let Some(path) = &self.result_path {
            if let Err(err) = std::fs::write(path, summary.to_json()) {
                eprintln!(
                    "[fandhe-egui-proxy][frame-metrics] 結果ファイルの書き込みに失敗した ({path:?}): {err}"
                );
            }
        }
    }
}

impl eframe::App for FrameMetricsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // vsync ペーシング下で連続再描画を駆動する（外部入力が無い環境
        // ＝実機・Xvfb でも再描画イベントが自然発生しないため必須）。
        ctx.request_repaint();

        let was_finished = self.collector.summary().is_some();
        self.collector.record_frame(std::time::Instant::now());

        let next_tick = self.store.state().tick.get().wrapping_add(1);
        self.store.state().tick.set(next_tick);

        egui::CentralPanel::default().show(ctx, |ui| {
            crate::render_screen(ui, &self.screen, &mut self.store);
        });

        if let Some(summary) = self.collector.summary().cloned() {
            if !was_finished {
                self.on_measurement_complete(&summary);
            }
            egui::Window::new("frame-metrics").show(ctx, |ui| {
                ui.label(format!("samples: {}", summary.sample_count));
                ui.label(format!("p50: {:.2} ms", summary.p50_ms));
                ui.label(format!("p95: {:.2} ms", summary.p95_ms));
                ui.label(format!("max: {:.2} ms", summary.max_ms));
                ui.label(format!(
                    "over 16.6ms: {:.2}%",
                    summary.over_budget_ratio * 100.0
                ));
                ui.label(summary.to_json());
            });
        }
    }
}

/// デスクトップ相当（native / glow バックエンド）向けの計測アプリ起動エントリ。
///
/// 呼び出し元は `main.rs` の feature 分岐（`frame-metrics` 有効時）のみ
/// （モジュール doc の「feature ゲート契約」参照）。
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
pub fn run_desktop() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([480.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "fandhe-egui-proxy-frame-metrics",
        native_options,
        Box::new(|cc| Ok(Box::new(FrameMetricsApp::new(cc, None)) as Box<dyn eframe::App>)),
    )
}

/// iOS（wgpu/Metal）向けの計測アプリ起動エントリ。
///
/// 呼び出し元は `ios.rs` の `run` 内 feature 分岐のみ。
#[cfg(target_os = "ios")]
pub fn run_ios() -> eframe::Result<()> {
    eframe::run_native(
        "fandhe-egui-proxy-frame-metrics",
        crate::ios::metal_native_options(),
        Box::new(|cc| Ok(Box::new(FrameMetricsApp::new(cc, None)) as Box<dyn eframe::App>)),
    )
}

/// Android（wgpu/Vulkan、NativeActivity）向けの計測アプリ起動エントリ。
///
/// 呼び出し元は `android.rs` の `android_main` feature 分岐のみ。`android.rs` と同様に
/// Vulkan 固定（REQ-1）を維持する。`AndroidApp::internal_data_path()` 配下へ結果 JSON を
/// 書き出す（`adb shell run-as <package> cat files/frame-metrics.json` で回収する運用、
/// `scripts/android/measure-frame-rate.sh` 参照）。取得できない場合は空パス
/// （カレントディレクトリ相対）へフォールバックし、書き込み失敗は
/// [`FrameMetricsApp::on_measurement_complete`] 側で panic にせず stderr へ記録する
/// （panic を API 契約にしない、coding-rust.md 準拠）。
#[cfg(target_os = "android")]
pub fn run_android(app: winit::platform::android::activity::AndroidApp) {
    let result_path = app
        .internal_data_path()
        .unwrap_or_default()
        .join("frame-metrics.json");

    let native_options = eframe::NativeOptions {
        android_app: Some(app),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(
                eframe::egui_wgpu::WgpuSetupCreateNew {
                    instance_descriptor: eframe::wgpu::InstanceDescriptor {
                        backends: eframe::wgpu::Backends::VULKAN,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            ..Default::default()
        },
        ..Default::default()
    };

    if let Err(err) = eframe::run_native(
        "fandhe-egui-proxy-frame-metrics",
        native_options,
        Box::new(move |cc| {
            Ok(
                Box::new(FrameMetricsApp::new(cc, Some(result_path.clone())))
                    as Box<dyn eframe::App>,
            )
        }),
    ) {
        eprintln!("[fandhe-egui-proxy][frame-metrics] Android 起動に失敗: {err:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 昇順の等差サンプルで p50 が中央値と一致することを確認する
    /// （偶数個・線形補間の基本ケース）。
    #[test]
    fn percentile_p50_matches_median_for_even_length() {
        let samples: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(percentile(&samples, 50.0), 5.5);
    }

    /// p0 / p100 がそれぞれ最小値・最大値と一致することを確認する（境界値）。
    #[test]
    fn percentile_boundaries_match_min_and_max() {
        let samples = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&samples, 0.0), 1.0);
        assert_eq!(percentile(&samples, 100.0), 4.0);
    }

    /// 空スライスは panic せず `0.0` を返す（安全側フォールバック）。
    #[test]
    fn percentile_empty_returns_zero() {
        assert_eq!(percentile(&[], 95.0), 0.0);
    }

    /// 単調に 16.6ms を超えるサンプル列で超過率が正しく算出されることを確認する
    /// （REQ-5 受け入れ基準「フレーム落ち 5% 未満」の判定対象そのもの）。
    #[test]
    fn summarize_computes_over_budget_ratio() {
        // 20 サンプル中 5 サンプル（25%）が予算超過になるよう構成する。
        let mut samples = vec![10.0; 15];
        samples.extend(vec![20.0; 5]);

        let summary = summarize(&samples, FRAME_BUDGET_MS);

        assert_eq!(summary.sample_count, 20);
        assert!((summary.over_budget_ratio - 0.25).abs() < f64::EPSILON);
        assert_eq!(summary.max_ms, 20.0);
    }

    /// 全サンプルが予算内なら超過率 0.0 になることを確認する（達成ケース）。
    #[test]
    fn summarize_zero_ratio_when_all_within_budget() {
        let samples = vec![8.0; 100];
        let summary = summarize(&samples, FRAME_BUDGET_MS);
        assert_eq!(summary.over_budget_ratio, 0.0);
    }

    /// `to_json` が数値フィールドを含む単純な JSON 文字列を返すことを確認する
    /// （外部出力経路 (b)(c) の形式契約）。
    #[test]
    fn to_json_contains_expected_fields() {
        let summary = FrameMetricsSummary {
            sample_count: 600,
            p50_ms: 16.0,
            p95_ms: 16.5,
            max_ms: 17.2,
            over_budget_ratio: 0.01,
        };
        let json = summary.to_json();
        assert!(json.contains("\"sample_count\":600"));
        assert!(json.contains("\"p95_ms\":16.500"));
    }

    /// warmup 破棄フレーム数（[`WARMUP_FRAMES`]）分は経過してもサンプルへ加算されず、
    /// 収集後 [`SAMPLE_FRAMES`] 分を集めた時点で `summary` が確定することを確認する。
    #[test]
    fn collector_finishes_after_warmup_and_sample_frames() {
        let mut collector = FrameMetricsCollector::new();
        let start = std::time::Instant::now();

        // 初回呼び出しは `last_instant` の初期化のみで経過時間を記録しないため、
        // 必要フレーム数 + 1 回呼ぶ。
        for i in 0..=(WARMUP_FRAMES as usize + SAMPLE_FRAMES) {
            let now = start + std::time::Duration::from_millis((i as u64) * 16);
            collector.record_frame(now);
        }

        let summary = collector.summary();
        assert!(summary.is_some());
        assert_eq!(summary.map(|s| s.sample_count), Some(SAMPLE_FRAMES));
    }
}
