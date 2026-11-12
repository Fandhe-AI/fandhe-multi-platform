//! 機能 crate「集中モードの状態」（focus-mode-status）。契約（`contract`）・失敗の種類（`error`）・
//! effect ドライバ（`driver`）・OS 別実装（`android`・`windows`、`android/java/*`）・機能宣言
//! （`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める（REQ-21 / TASK-21.4、Issue #1189。
//! 移行の手順は `docs/operations/capability-crate-migration.md`）。iOS・macOS は OS が公開しないため実装が無い。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`focus_mode_status_show_effect`] を
//! 画面（`fandhe_core::screens::focus_mode_status_show`）の effect として登録する。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 [`platform_focus_mode_status`] は状態型に依存しない。
//!
//! 守る範囲と守らない範囲は `contract.rs` の契約を参照。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::FocusModeStatusError;
