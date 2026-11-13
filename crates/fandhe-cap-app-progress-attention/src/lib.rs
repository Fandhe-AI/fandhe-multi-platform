//! app-progress-attention（Dock・タスクバーの進捗表示と注意喚起）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`app_progress_attention_show_effect`）。macOS（`NSDockTile` /
//! `requestUserAttention`）と Windows（`ITaskbarList3` / `FlashWindowEx`）が実装し、他の OS は `Unsupported`（モバイルに該当機能は無い）。
//! 注意喚起の前面判定は `fandhe_runtime::HostView::focused` が返す OS の観測を使い、アプリ内の記憶やタイマーは持たない。
//! 守る範囲は範囲外の進捗値を OS へ渡さないこと・失敗を成功に畳まないこと。守らない範囲は
//! `TaskbarButtonCreated` 前の呼び出しの挙動と複数ウィンドウのグループ表示。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_app_progress_attention()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "macos", target_os = "windows"), deny(unsafe_code))]

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
pub use error::AppProgressAttentionError;
