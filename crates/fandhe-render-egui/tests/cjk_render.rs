//! 同梱 CJK フォント（Noto Sans JP）適用後の実描画検証（TASK-10.1 / REQ-10 / M3）。
//!
//! `desktop_render.rs`（TASK-1.2）と同じ `egui_kittest`（wgpu バックエンド）の枠組みで
//! `sample_screen()` を描画するが、本ファイルは描画前に必ず
//! [`fandhe_egui_proxy::font::install_cjk_font`] を通す点が異なる（`ProxyApp::new` と
//! 同じ配線を `Harness` 経由で再現する）。日本語ラベル（「メモ」「カウントを増やす」等）が
//! tofu 表示でないことを PNG エビデンスとして `CARGO_TARGET_TMPDIR` へ保存し、
//! REQ-10 受け入れ基準（CJK 対応フォントを既定の同梱フォントセットに含め、日本語テキストが
//! 正しくレンダリングされることを確認する）の目視確認材料とする。
//!
//! GPU アダプタが存在しない環境でのスキップ判定は `desktop_render.rs` の
//! `is_adapter_unavailable_panic` パターンを踏襲する（重複コメントは避け、
//! 判定の詳細はそちらのモジュールコメントを参照）。ここでの PNG 保存はあくまで
//! 目視確認用の補助エビデンスであり、フォントが実際に解釈・ラスタライズ可能であることの
//! 機械的な証明は `fandhe-fonts` の `tests/glyph_coverage.rs`（GPU 非依存）が担う。

use egui_kittest::kittest::Queryable;
use fandhe_core::{sample_screen, SampleState, StateStore};

/// GPU アダプタの取得を複数スレッドから同時に行うとハングする既知の制約のため、
/// テスト単位で直列化する（`desktop_render.rs` と同じ理由）。
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// プロセス内で一度「アダプタなし」と判定したら以降は `Harness` の再構築を省略する。
static GPU_UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `ProxyApp::new` と同じ配線（`install_cjk_font` 適用後に `sample_screen()` を描画）を
/// `Harness` 経由で再現する。
///
/// `install_cjk_font` は `ctx.set_fonts` を都度呼ぶため、`Harness` が毎フレーム呼び出す
/// 描画クロージャの中で無条件に呼ぶと初回以降は無駄な再登録になる。`Once` で初回フレームのみ
/// 適用するが、本テストファイルには `Harness` を個別に構築するテストが 2 件あり、
/// 各 `Harness` は自分専用の `egui::Context` を持つ（`set_fonts` はコンテキスト単位の状態）。
/// `Once` を関数外の static にすると「プロセス内で最初に実行された 1 つの `Harness` にしか
/// フォントが適用されない」不具合（2 件目のテストが未適用の既定フォントのまま実行され、
/// テスト実行順序に結果が依存する）を招くため、`build_harness` 呼び出しごとに新しい
/// `Once` をクロージャへキャプチャさせ、`Harness`（= `Context`）ごとに独立させる。
fn build_harness() -> egui_kittest::Harness<'static, StateStore<SampleState>> {
    let screen = sample_screen();
    let install_font_once = std::sync::Once::new();

    egui_kittest::Harness::builder()
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<SampleState>| {
                install_font_once.call_once(|| {
                    fandhe_egui_proxy::font::install_cjk_font(ui.ctx());
                });
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(SampleState::default()),
        )
}

/// アダプタ欠如由来の panic メッセージにのみ現れる部分文字列
/// （`desktop_render.rs` の `ADAPTER_UNAVAILABLE_PANIC_MARKERS` と同一の判定基準）。
const ADAPTER_UNAVAILABLE_PANIC_MARKERS: [&str; 2] =
    ["CustomNativeAdapterSelectionError", "No adapter found"];

fn is_adapter_unavailable_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    let message: Option<&str> = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));

    match message {
        Some(message) => ADAPTER_UNAVAILABLE_PANIC_MARKERS
            .iter()
            .any(|marker| message.contains(marker)),
        None => false,
    }
}

/// `build_harness` を呼び出し、wgpu アダプタ欠如による構築時 panic のみを捕捉して
/// `None` を返す（テストをスキップさせる）。判定に一致しない panic は
/// 再送出してテストを失敗させる（fake-green 化を防ぐ、`desktop_render.rs` と同方針）。
fn build_harness_or_skip() -> Option<egui_kittest::Harness<'static, StateStore<SampleState>>> {
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build_harness());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、CJK 実描画テストをスキップします（既知・別テストで確認済み）"
        );
        return None;
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build_harness)) {
        Ok(harness) => Some(harness),
        Err(payload) => {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                std::panic::resume_unwind(payload);
            }

            GPU_UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!(
                "notice: wgpu アダプタが取得できない環境のため、CJK 実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// 同梱フォント適用後も `sample_screen()` の日本語ラベルを含む全要素が
/// AccessKit ツリーに存在することを検証する（フォント差し替えが要素欠落を
/// 起こさないことの回帰確認。文字の見た目自体は次の PNG エビデンステストで確認する）。
#[test]
fn all_elements_are_present_after_installing_bundled_cjk_font() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("サンプル画面");
    harness.get_by_label("代表 1 画面サンプル");
    harness.get_by_label("メモ");
    harness.get_by_label("カウントを増やす");
    harness.get_by_label("項目を追加");
}

/// 同梱フォント適用後の実レンダリング結果を PNG として保存し、日本語ラベルが
/// tofu 表示でないことの目視確認材料とする。
///
/// 出力先は `CARGO_TARGET_TMPDIR`（`target/` 配下の使い捨てディレクトリ）とする
/// （`desktop_render.rs` の `renders_to_png_evidence_for_verification_report` と同方針）。
/// エビデンス画像が必要な場合は、実行後に `CARGO_TARGET_TMPDIR` 配下の出力を各自で控える。
#[test]
fn renders_to_png_evidence_with_bundled_cjk_font() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    let image = harness
        .render()
        .expect("wgpu レンダリングが失敗した（デスクトップ相当環境の GPU/Vulkan 前提が未整備）");

    let out_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(out_dir).expect("エビデンス出力ディレクトリの作成に失敗した");

    image
        .save(out_dir.join("cjk-render-noto-sans-jp.png"))
        .expect("PNG 保存に失敗した");
}
