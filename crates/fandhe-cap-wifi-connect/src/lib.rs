//! wifi-connect の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。指定した SSID（とパスワード）の Wi-Fi への接続を OS に依頼する。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`wifi_connect_show_effect`）。SSID・パスワードはログ・エラー・`Debug` に出さず、trim しない（秘密値）。
//! 守る範囲は 1 プロセス内の要求の順序・失敗を成功と見せないこと・Activity 破棄時の保留要求の確定、守らない範囲は接続の維持と OS のダイアログの表示有無
//! （`contract` の doc を参照）。Android・macOS・Windows が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_wifi_connect()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
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
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::WifiConnectError;
