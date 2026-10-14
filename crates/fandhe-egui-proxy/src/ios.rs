//! iOS 向け起動エントリ（TASK-1.3-S2 / REQ-1 / M3）。
//!
//! PoC-1（`docs/spec/03-poc/ui-rendering/README.md`）で確認したとおり、glow バックエンド
//! （`glutin` 経由の GL コンテキスト生成）は iOS 非対応のため、iOS ターゲットのみ
//! wgpu（Metal）バックエンドへ切り替える。本モジュールはその切り替え先の初期化コードを担い、
//! `Cargo.toml` 側のターゲット条件分岐（`cfg(target_os = "ios")`、TASK-1.3-S1 / #40）と対になる。
//!
//! 呼び出し元は `main.rs` の iOS 向け `fn main`（本モジュールの [`run`] へ委譲するのみ）。
//! デスクトップ用の `FANDHE_PROXY_AUTOSHOT`（環境変数経由の自動撮影）経路はここでは配線しない
//! （外部入力面を増やさない方針。security.md A03 対策）。
//!
//! 実機/シミュレータでの実描画・要素欠落確認は TASK-1.4（#229、実機環境整備後）で行う。
//! 本モジュールの完了条件は初期化コードの実装と `aarch64-apple-ios` 向けコンパイル検証まで。
//!
//! 本モジュール自体は `lib.rs` 側の `#[cfg(target_os = "ios")] pub mod ios;` で
//! ゲートされるため、モジュール内でのターゲット条件の再指定はしない。

/// `run` が組み立てる `eframe::App` の `egui::Context`（`cc.egui_ctx.clone()`）を
/// 保持する（改訂 4、`android.rs::EGUI_CTX` と同型）。`device/ios.rs` の
/// `fandhe_rust_file_picker_outcome`（`UIDocumentPickerViewController` の delegate
/// コールバック、UI スレッド外）は `bridge::deliver` で結果を置くだけでは egui の
/// 描画ループを起こせない（`file_pick.rs::drive_mobile` は pull 型で `sender` も
/// 画面の `generation` も持たず、毎フレームの effect 呼び出しでしか結果を拾えない）
/// ため、結果を置いたら [`request_repaint`] で確実に再描画を起こす。
///
/// `frame-metrics` 版の `run`（`frame_metrics::run_ios` へ委譲）では設定しない。
/// 計測専用で file_pick 画面を使わないため。
static EGUI_CTX: std::sync::Mutex<Option<egui::Context>> = std::sync::Mutex::new(None);

/// [`EGUI_CTX`] を読み、確保済みなら `request_repaint` を呼ぶ。`run` 実行前
/// （通常到達しない）は何もしない。
pub(crate) fn request_repaint() {
    if let Some(ctx) = EGUI_CTX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        ctx.request_repaint();
    }
}

/// iOS 向けに wgpu（Metal）バックエンドを明示指定した `NativeOptions` を構築する。
///
/// `wgpu::Backends::METAL` のみを許可することで、iOS では選択され得ない Vulkan / GL 等の
/// バックエンドへ `wgpu` が誤ってフォールバックしないようにする（初期化失敗時の原因切り分けを
/// 容易にする狙い。iOS 実機/シミュレータは Metal のみが有効なため機能面での制約にはならない）。
pub fn metal_native_options() -> eframe::NativeOptions {
    let wgpu_setup = eframe::egui_wgpu::WgpuSetupCreateNew {
        instance_descriptor: eframe::wgpu::InstanceDescriptor {
            backends: eframe::wgpu::Backends::METAL,
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::NativeOptions {
        // iOS はフルスクリーン前提のため、デスクトップ側（main.rs）のような
        // 固定ウィンドウサイズ指定は行わず、既定のビューポート設定に委ねる。
        viewport: egui::ViewportBuilder::default(),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(wgpu_setup),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// iOS 向けエントリポイント。`ProxyApp::new` を wgpu（Metal）バックエンドで起動する。
///
/// 呼び出し元は `main.rs` の iOS 向け `fn main` のみを想定する
/// （Xcode 組み込み用の FFI エクスポートは TASK-1.4 / TASK-11.1 側の検討事項であり、
/// 本関数は `#![forbid(unsafe_code)]` を維持したまま Rust 関数として呼ばれる前提）。
///
/// TASK-5.1（Issue #236）でコールドスタート計測用に [`crate::cold_start::ColdStartProbe`]
/// でラップする経路を `cold-start-probe` feature（既定 off）の下に追加した。feature 有効時
/// （`scripts/ios/measure-cold-start.sh` ビルド時のみ）にこの実装が使われ、
/// `ProxyApp` 自体の初期化・描画ロジックは変更していない
/// （TASK-1.4 の実描画確認結果に対する後方互換）。
///
/// `t0` は本関数の呼び出し（`fn main`（iOS 向け、`main.rs`）から実質最速で到達する地点）。
/// `close_after_log` は false を渡し、既存挙動（ログ出力後もアプリを継続動作させ、
/// 終了は実機/シミュレータのライフサイクルに委ねる）を維持する
/// （デスクトップ native 側の計測エントリ（`main.rs` の `cold-start-probe` 分岐の `fn main`）とは
/// この点のみ異なる。共通ロジックは `cold_start.rs` モジュール doc 参照、Issue #593）。
///
/// `frame-metrics`（TASK-5.3 / Issue #238）とは独立した計測専用 feature であり、
/// 両方を同時に有効化した場合は本経路（`cold-start-probe`）を優先する
/// （`Cargo.toml` の `[features]` コメント参照。通常のビルドでは両 feature とも
/// 無効のため、本番経路には影響しない）。
#[cfg(feature = "cold-start-probe")]
pub fn run() -> eframe::Result<()> {
    let start = std::time::Instant::now();
    eframe::run_native(
        "fandhe-egui-proxy",
        metal_native_options(),
        Box::new(move |cc| {
            // 改訂 4: ファイル選択 FFI 受け口からの `request_repaint` 経路確保のため
            // static へ複製する（[`EGUI_CTX`] doc 参照）。
            *EGUI_CTX.lock().unwrap_or_else(|e| e.into_inner()) = Some(cc.egui_ctx.clone());
            let app =
                crate::cold_start::ColdStartProbe::new(crate::ProxyApp::new(cc), start, false);
            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
}

/// TASK-5.3（REQ-5 / M4、Issue #238）: `frame-metrics` feature 有効時（かつ
/// `cold-start-probe` 無効時）は `crate::frame_metrics::run_ios`（計測専用アプリ）へ
/// 委譲する。既存の `ProxyApp` 経路（両 feature 無効時）は変更しない
/// （`frame_metrics.rs` モジュール doc 参照）。
#[cfg(all(feature = "frame-metrics", not(feature = "cold-start-probe")))]
pub fn run() -> eframe::Result<()> {
    crate::frame_metrics::run_ios()
}

/// iOS 向けエントリポイント（本番経路。両計測用 feature 無効時、既定）。
///
/// 計測専用ラッパー（`ColdStartProbe` / `frame_metrics::run_ios`）を経由せず、
/// レジストリ（`screens::build_app`、Issue #715）で解決した画面（未指定・未登録時は
/// `ProxyApp::new` の sample 画面）を直接起動する。ストア配布ビルドは既定でこの実装が
/// 使われ、計測用の `eprintln!`（`COLD_START_MS=`）等は一切コンパイルに含まれない
/// （Review 指摘対応）。
///
/// Issue #795: 実描画 App を [`crate::ios_text_input::TextInputApp`] で包み、日本語
/// ソフトキーボードの変換・確定文字列を egui へ届ける（`ios_text_input.rs` モジュール
/// doc 参照）。`cold-start-probe`/`frame-metrics`（計測専用ビルド）はこのラッパーを
/// 経由しない（計測結果への影響を避ける）。
#[cfg(not(any(feature = "cold-start-probe", feature = "frame-metrics")))]
pub fn run() -> eframe::Result<()> {
    eframe::run_native(
        "fandhe-egui-proxy",
        metal_native_options(),
        Box::new(move |cc| {
            // 改訂 4: ファイル選択 FFI 受け口からの `request_repaint` 経路確保のため
            // static へ複製する（[`EGUI_CTX`] doc 参照）。
            *EGUI_CTX.lock().unwrap_or_else(|e| e.into_inner()) = Some(cc.egui_ctx.clone());
            let app: Box<dyn eframe::App> = match crate::screens::selected_app_name()
                .and_then(|name| crate::build_app(&name, cc))
            {
                Some(app) => app,
                None => Box::new(crate::ProxyApp::new(cc)),
            };
            Ok(Box::new(crate::ios_text_input::TextInputApp::new(app)))
        }),
    )
}
