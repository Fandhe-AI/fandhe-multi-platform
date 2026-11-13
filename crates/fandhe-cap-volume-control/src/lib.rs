//! volume-control の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。端末のシステム音量（メディア音量）の読み取りと 1 段階の変更。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`volume_control_show_effect`）。守る範囲は 1 プロセス内の順序・失敗を成功と見せないこと、
//! 守らない範囲は他アプリによる音量変更への追従（driver が 1 秒ごとに読み直す）。
//! iOS は読み取りのみ（変更は OS のスライダー）、Android・macOS・Windows が読み取りと変更、Web は非対応（`contract` の doc を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_volume_control()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
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
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::VolumeControlError;
