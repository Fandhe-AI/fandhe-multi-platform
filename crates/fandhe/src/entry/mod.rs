//! 各 OS の入口の中身（Issue #1200、REQ-21 / TASK-21.5）。`#[fandhe::main]`（`fandhe-macros`）が生成するコードから呼ぶ。
//!
//! 利用者は直接呼ばない（`fandhe::__private` として doc から隠す）。生成コードが `::fandhe::__private::…` を
//! 名指しするため、パスは変えない。
//!
//! - デスクトップ（macOS・Windows・Linux）: [`run`]（glow）
//! - iOS: [`run`]（wgpu / Metal、`ios.rs`）
//! - Android: [`run_android`]（wgpu / Vulkan、`android.rs`）
//! - Web: [`start_web`]（glow / WebGL2、`web.rs`）
//!
//! どの OS でも、利用者の関数（[`BuildFn`]）を起動時に 1 回だけ呼び、返した画面を最初に出す。画面の名前や
//! 環境変数では選ばない（画面の登録表に依存しない）。選んだ端末機能の起動前の登録は `lifecycle.rs` が持つ。終了処理と
//! 再生成時の破棄は、機能 crate が `fandhe_runtime::teardown` へ登録したものを描画層と `android.rs` が回す。

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;
// `soft_keyboard` の純関数は Android と iOS の両方が使い、ホストでも単体テストする。
#[cfg(any(target_os = "ios", test))]
mod ios_text_input;
#[cfg(not(target_arch = "wasm32"))]
mod lifecycle;
#[cfg(any(target_os = "android", target_os = "ios", test))]
mod soft_keyboard;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_os = "android")]
pub use android::AndroidApp;
/// 生成コードが `#[wasm_bindgen(start, wasm_bindgen = …)]` に渡すパス。利用者が wasm-bindgen に依存しなくて済む。
#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen;

use fandhe_render_egui::app::ProxyAppDyn;

/// 利用者の画面を作る関数（`#[fandhe::main]` を付けた関数）。
pub type BuildFn = fn(&egui::Context) -> Box<dyn ProxyAppDyn>;

/// デスクトップと iOS の起動。`title` はウィンドウ名（デスクトップ）とアプリ名（eframe の永続化の鍵）。
#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
pub fn run(title: &str, build: BuildFn) -> Result<(), fandhe_render_egui::LaunchError> {
    #[cfg(target_os = "ios")]
    return ios::run(title, build);
    #[cfg(not(target_os = "ios"))]
    {
        lifecycle::before_launch();
        fandhe_render_egui::launch::run_desktop(title, desktop_inner_size(), move |lc| {
            // Issue #898: Windows の共有シート・生体認証などが要る HWND は起動時のコンテキストからしか取れないため、
            // 構築時に 1 度だけ控える。
            #[cfg(target_os = "windows")]
            fandhe_platform_windows::remember_window(lc);
            let app: Box<dyn fandhe_render_egui::EguiApp> = build(lc.egui_ctx());
            app
        })
    }
}

/// デスクトップの初期ウィンドウの大きさ。window-management を選んだときは、その「既定の大きさへ戻す」と同じ値で
/// 始める（大きさの正本は `fandhe_core::screens::window_management_show::DEFAULT_INNER_SIZE`）。それ以外は eframe の既定。
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
fn desktop_inner_size() -> Option<[f32; 2]> {
    #[cfg(feature = "window-management")]
    return Some(fandhe_core::screens::window_management_show::DEFAULT_INNER_SIZE);
    #[cfg(not(feature = "window-management"))]
    None
}

/// Android の起動（`android_main` から呼ぶ）。
#[cfg(target_os = "android")]
pub fn run_android(app: AndroidApp, title: &str, build: BuildFn) {
    android::run(app, title, build);
}

/// Web の起動（`#[wasm_bindgen(start)]` から呼ぶ）。
#[cfg(target_arch = "wasm32")]
pub fn start_web(build: BuildFn) -> Result<(), wasm_bindgen::JsValue> {
    web::start(build)
}

/// `#[fandhe::main]` を使わずに独自の起動をする最終成果物（計測専用の入口を持つカタログ）が使う、起動前の端末機能の登録。
#[cfg(not(target_arch = "wasm32"))]
pub fn before_launch() {
    lifecycle::before_launch();
}
