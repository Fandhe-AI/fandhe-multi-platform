//! 画面カタログ `fandhe-gallery` のデスクトップ（macOS・Windows・Linux）・iOS の実行体（TASK-1.2、Issue #1165）。
//!
//! 通常は `lib.rs` の `#[fandhe::main]` が生成した [`fandhe_gallery::main`] を呼ぶだけ（入口の中身は `fandhe`）。
//! Android・Web では生成された `main` は何もしない（入口は cdylib の `android_main`・`#[wasm_bindgen(start)]`）。
//!
//! `FANDHE_PROXY_AUTOSHOT=<path>`（デスクトップ）を指定すると数フレーム後に撮影して PNG を保存し、ウィンドウを閉じる
//! （`select.rs`）。保存に失敗したら非ゼロで終了する（`xvfb-run` 上での実描画確認・エビデンス取得用）。
//!
//! 計測専用ビルド（feature `cold-start-probe`・`frame-metrics`、TASK-5.1・5.3）は入口ごと差し替える。
//! 両方を有効にしたら `cold-start-probe` を優先する。

#[cfg(not(any(feature = "cold-start-probe", feature = "frame-metrics")))]
fn main() -> Result<(), fandhe_render_egui::LaunchError> {
    fandhe_gallery::main()?;
    #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
    if fandhe_gallery::autoshot_failed() {
        eprintln!("[fandhe-gallery] 自動撮影の PNG 保存に失敗したため、非ゼロで終了する。");
        std::process::exit(1);
    }
    Ok(())
}

/// コールドスタート計測（デスクトップ）。計測後に自動でウィンドウを閉じてプロセスを終える（複数回試行して中央値を取る運用、
/// Issue #593）。ウィンドウの大きさとバックエンドは通常起動と揃える。
#[cfg(all(
    not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")),
    feature = "cold-start-probe"
))]
fn main() -> Result<(), fandhe_render_egui::LaunchError> {
    let start = std::time::Instant::now();
    fandhe_render_egui::launch::run_desktop(
        "fandhe-gallery",
        Some(fandhe_core::screens::window_management_show::DEFAULT_INNER_SIZE),
        move |lc| {
            Box::new(fandhe_gallery::demo::cold_start::ColdStartProbe::new(
                fandhe_render_egui::ProxyApp::new(lc.egui_ctx()),
                start,
                true,
            ))
        },
    )
}

/// コールドスタート計測（iOS）。ログ出力後もアプリを続け、終了は OS のライフサイクルに任せる。
/// 計測の起点は起動前の登録の後（移行前の `ios::run` と同じ位置。計測結果の比較可能性を保つ）。
#[cfg(all(target_os = "ios", feature = "cold-start-probe"))]
fn main() -> Result<(), fandhe_render_egui::LaunchError> {
    fandhe::__private::before_launch();
    let start = std::time::Instant::now();
    fandhe_render_egui::launch::run_ios("fandhe-gallery", move |lc| {
        fandhe_runtime::set_repaint_waker(std::sync::Arc::new(
            fandhe_render_egui::app::EguiWaker::new(lc.egui_ctx().clone()),
        ));
        Box::new(fandhe_gallery::demo::cold_start::ColdStartProbe::new(
            fandhe_render_egui::ProxyApp::new(lc.egui_ctx()),
            start,
            false,
        ))
    })
}

/// フレームレート計測（デスクトップ）。
#[cfg(all(
    not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")),
    feature = "frame-metrics",
    not(feature = "cold-start-probe")
))]
fn main() -> Result<(), fandhe_render_egui::LaunchError> {
    fandhe_gallery::demo::frame_metrics::run_desktop()
}

/// フレームレート計測（iOS）。起動前の端末機能の登録だけは通常起動と揃える。
#[cfg(all(
    target_os = "ios",
    feature = "frame-metrics",
    not(feature = "cold-start-probe")
))]
fn main() -> Result<(), fandhe_render_egui::LaunchError> {
    fandhe::__private::before_launch();
    fandhe_gallery::demo::frame_metrics::run_ios()
}

/// 計測専用ビルドの Android・Web では bin は何もしない（Android の計測は `lib.rs` の `android_main`）。
#[cfg(all(
    any(target_os = "android", target_arch = "wasm32"),
    any(feature = "cold-start-probe", feature = "frame-metrics")
))]
fn main() {}
