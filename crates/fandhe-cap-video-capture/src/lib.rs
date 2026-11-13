//! video-capture の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。OS の撮影 UI（Windows はアプリが組むパネル）で動画を 1 本撮る。
//!
//! 呼び出し元は画面側の effect（`driver`）とフレームワーク外のアプリ（`platform_video_capture()`）。
//! 守る範囲は、1 プロセス内の要求の順序・失敗や 0 バイトを成功表示へ畳まないこと・一時ファイルの解放。
//! 守らない範囲は、`EXTRA_OUTPUT` を無視して別の場所へ保存するカメラアプリ（空のファイルとして失敗にする）、
//! 撮影中の背景遷移の扱い（OS 任せ）、撮影中のプロセス死（詳細は `contract`）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ（Windows の撮影パネルの状態型を除く）。
//! 入口 `pub fn platform_video_capture()` は状態型に依存しないため、フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

// 各 OS の実装はホストのテストでも写像の検証だけ走らせる（実体の FFI は各 OS の cfg の内側）。
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
pub use error::VideoCaptureError;
