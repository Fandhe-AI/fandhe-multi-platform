//! 機能 crate「予定の追加」（calendar）。契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・
//! OS 別実装（`apple`＝EventKit の FFI、`apple/*.m`、`android`＝カレンダーアプリの編集画面を開く JNI、
//! `android/java/*`）・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1187。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`calendar_add_effect`] を画面
//! （`fandhe_core::screens::calendar_add`）の effect として登録する。Activity 再生成では gallery の
//! `android_main` から Android 限定の `reset_pending_tracking` を呼ぶ。egui・`fandhe-device` には依存しない。
//! 画面の外からは、OS の完了まで待つ同期の [`platform_calendar`] と、待ちを打ち切れる [`begin_add_event`] /
//! [`poll_add_event`] / [`discard_add_event`] を使う（打ち切った要求は OS 側で後から保存されうる）。
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
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;

pub use contract::*;
#[cfg(target_os = "android")]
pub use driver::reset_pending_tracking;
pub use driver::{begin_add_event, discard_add_event, platform_calendar, poll_add_event};
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::CalendarError;
