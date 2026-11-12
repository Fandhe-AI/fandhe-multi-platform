//! 機能 crate「通話 UI 統合」（call-ui）。契約（`contract`）・effect ドライバ（`driver`）・OS 別実装
//! （`android`＝self-managed ConnectionService と着信通知、`ios`＝CallKit の FFI、`ios/*.m`、`android/java/*`）・
//! 機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1187。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`call_ui_show_effect`] を画面
//! （`fandhe_core::screens::call_ui_show`）の effect として登録する。Activity 再生成では gallery の
//! `android_main` から [`reset_pending_tracking`] を呼ぶ。egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は [`CallUi`] の契約（`contract.rs`）を参照。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
