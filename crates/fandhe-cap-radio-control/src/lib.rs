//! radio-control の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。無線（Wi-Fi・Bluetooth）の状態の読み取りと切り替えを OS に依頼する。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`radio_control_show_effect`）と、Activity 再生成時の
//! `reset_pending_tracking`（gallery の `android_main` から、Android のみ）。守る範囲は 1 プロセス内の要求の順序・
//! 失敗を成功と見せないこと・Activity 破棄時の保留要求の確定、守らない範囲は設定画面での利用者の操作・読み直し時点の
//! 切り替わり途中の値・ダイアログの待ち時間（`contract` の doc を参照）。Android・macOS・Windows が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_radio_control()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "macos"), deny(unsafe_code))]

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
pub use error::RadioControlError;
