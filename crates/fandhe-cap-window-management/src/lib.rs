//! window-management の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1334）。ウィンドウの位置・大きさ・最前面を依頼し、読み戻しで確かめる。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`window_management_show_effect`）と、Web の `fandhe-web-dom`（契約の純関数だけを共有）。
//! 書き込みと読み戻しは描画層の `fandhe_runtime::HostView`（`request_window` / `window_state`）で行い、その口に無い最前面と作業領域だけを
//! macOS・Windows の OS 実装が読む。iOS・Android は対応表で対象外のため `Unsupported`。
//! 守る範囲は依頼と完了の分離・全画面中の拒否・嘘の成功の禁止、守らない範囲は複数ウィンドウ・倍率の違う画面をまたぐ移動（`contract` の doc を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_window_management()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "macos", target_os = "windows"), deny(unsafe_code))]

mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::platform_window_management;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::WindowManagementError;
