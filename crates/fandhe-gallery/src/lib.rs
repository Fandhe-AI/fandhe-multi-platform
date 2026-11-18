//! fandhe-gallery: 全機能の画面を登録し、実機確認・撮影に使う非公開アプリ（`publish = false`）。
//!
//! 位置づけ（`docs/architecture/consumer-app-structure.md` §3、TASK-21.2 / Issue #1165）:
//! 画面の登録（[`screens`]）と、各 OS の最終成果物を持つ。各 OS の入口は `#[fandhe::main]`（Issue #1200）が
//! 本ファイルの [`app`] から生成する（利用者のアプリと同じ形）。
//!
//! - デスクトップ・iOS の実行体: `main.rs`（bin `fandhe-gallery-desktop`）が生成された [`main`] を呼ぶ
//! - Android の cdylib: 生成された `android_main`
//! - Web の wasm cdylib: 生成された `#[wasm_bindgen(start)]`
//!
//! 起動前の端末機能の登録・終了処理・Activity 再生成時の破棄は入口 crate `fandhe`（全機能の feature）が行う。
//! 本 crate が持つのは、画面を名前で選ぶ部分（`select.rs`）と、検証・計測専用のアプリ（[`demo`]）だけ。
//!
//! Android のみ `unsafe_code` を `deny` へ緩める（生成される `android_main` の `no_mangle` と、
//! debug システムプロパティの読み出し 1 箇所。`coding-rust.md` の FFI 例外規定）。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

pub mod demo;
pub mod screens;
mod select;
pub use screens::{build_app, registered_names};
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
pub use select::autoshot_failed;

/// 起動画面を作る。名前の解決は `select.rs`（`FANDHE_PROXY_APP` など）。
///
/// 計測専用ビルド（`frame-metrics`）は入口ごと差し替えるため、`#[fandhe::main]` を付けない
/// （`main.rs` と下の `android_main` が計測アプリを直接起動する）。
#[cfg_attr(not(feature = "frame-metrics"), fandhe::main)]
#[cfg_attr(feature = "frame-metrics", allow(dead_code))]
fn app(ctx: &egui::Context) -> Box<dyn fandhe_render_egui::ProxyAppDyn> {
    select::app(ctx)
}

/// Android の入口（計測専用ビルド）。`android-activity` がこの固定名で呼ぶ。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`no_mangle` は名前修飾を外すだけで、`android_main` は
/// `android-activity` の規約上の固定名。`frame-metrics` では `#[fandhe::main]` を付けないため重複しない。
#[cfg(all(target_os = "android", feature = "frame-metrics"))]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(app: fandhe::__private::AndroidApp) {
    demo::frame_metrics::run_android(app);
}
