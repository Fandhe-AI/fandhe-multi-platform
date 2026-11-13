//! display-refresh-hdr（画面のリフレッシュレート・HDR・広色域の読み取り）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`display_refresh_hdr_show_effect`）と、契約の純関数
//! （`require_any`・`DisplayCapabilities`）を共有する `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。
//! Android（`Display`）・iOS（`UIScreen`）・macOS（`NSScreen`）・Windows（DXGI）が押下ごとに 1 回読む。
//! 守る範囲は取れない項目を `None` にとどめ、全項目が取れないときは失敗にすること（嘘の成功を作らない）。
//! 守らない範囲は読み取り後の画面モード変更（次の押下で読み直す）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_display_refresh_hdr()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は iOS・macOS・Windows の FFI に限る（Android の JNI は共通層の `with_activity_class` が持つ。`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "ios", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::DisplayRefreshHdrError;
