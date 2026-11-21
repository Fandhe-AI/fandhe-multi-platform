//! iOS の入口（`#[fandhe::main]` が生成する `main` から呼ぶ。Issue #1200）。
//!
//! glow（GL）は iOS 非対応のため wgpu（Metal）で起動する（PoC-1、`fandhe_render_egui::launch::run_ios`）。
//! `UIApplicationMain` より前に、選んだ端末機能の登録（URL scheme・背景処理・背景転送の完了など、
//! 起動と同じターンで届く通知に間に合わせるもの）を済ませる（`super::lifecycle`）。
//! ソフトキーボードの確定・未確定文字列は [`super::ios_text_input`] が egui へ橋渡しする。

/// iOS アプリの本体。`build` が返した画面を最初に出す。戻らない（`UIApplicationMain`）。
pub(crate) fn run(
    title: &str,
    build: super::BuildFn,
) -> Result<(), fandhe_render_egui::LaunchError> {
    super::lifecycle::before_launch();
    fandhe_render_egui::launch::run_ios(title, move |lc| {
        let ctx = lc.egui_ctx();
        // 機能 crate が UIKit のコールバックから次フレームを起こす口（`fandhe_runtime::request_repaint`）。
        fandhe_runtime::set_repaint_waker(std::sync::Arc::new(
            fandhe_render_egui::app::EguiWaker::new(ctx.clone()),
        ));
        Box::new(super::ios_text_input::TextInputApp::new(build(ctx)))
    })
}
