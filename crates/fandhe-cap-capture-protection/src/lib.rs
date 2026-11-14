//! capture-protection（画面キャプチャの防止・検知）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! Windows は `SetWindowDisplayAffinity`、Android は `FLAG_SECURE`（防止）と API 34+ / 35+ の検知コールバック、
//! iOS は `UIScreen.isCaptured` と撮影通知（検知のみ）。macOS・Web は対象外で `Unsupported`。
//! `fandhe-gallery` の画面登録から effect `capture_protection_show_effect` が呼ばれ、要求の直後と 1 秒間隔で OS から読み戻す。
//!
//! 守る範囲: 1 プロセス内の順序、失敗を成功と見せないこと、期限後に適用しないこと、検知登録と解除の対応。
//! 守らない範囲: 背景中のスクリーンショット、adb・計装の撮影、外部カメラ、掛ける前に始まった録画。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_capture_protection()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "ios", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "ios", target_os = "windows"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::CaptureProtectionError;
