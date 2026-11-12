//! 機能 crate「予定の追加」（calendar）。契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・
//! OS 別実装（`apple`＝EventKit の FFI、`apple/*.m`、`android`＝カレンダーアプリの編集画面を開く JNI、
//! `android/java/*`）・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1187。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`calendar_add_effect`] を画面
//! （`fandhe_core::screens::calendar_add`）の effect として登録する。Activity 再生成では gallery の
//! `android_main` から Android 限定の `reset_pending_tracking` を呼ぶ。egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は `contract.rs` の契約を参照。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::CalendarError;
