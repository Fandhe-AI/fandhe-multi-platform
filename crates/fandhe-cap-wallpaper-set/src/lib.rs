//! wallpaper-set（壁紙・ロック画面の設定）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`wallpaper_set_effect`）と、Activity 再生成時の
//! `fandhe-gallery` の `android_main`（`reset_pending_tracking`）。Android（`WallpaperManager`）・macOS（`NSWorkspace`）・
//! Windows（`SystemParametersInfoW`）が実装し、他の OS は `Unsupported`。渡す画像は検査済みの PNG（`PngImage`）だけ。
//! 守る範囲は 1 プロセス内の順序・失敗や禁止を成功に畳まないこと・完了通知を 1 回だけ反映すること。
//! 守らない範囲は設定後に利用者や他アプリが壁紙を変えること。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_wallpaper_set()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は macOS・Windows の FFI と Android の JNI エクスポート（`nativeOn*`）に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::WallpaperSetError;
pub use fandhe_device::photo_library::PngImage;
