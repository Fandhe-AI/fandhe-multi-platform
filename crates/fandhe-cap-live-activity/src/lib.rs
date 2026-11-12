//! 機能 crate「ライブアクティビティ」（live-activity）。契約（`contract`）・失敗の種類（`error`）・effect ドライバ
//! （`driver`）・OS 別実装（`android`＝進行中通知、`ios`＝ActivityKit の FFI、`ios/*.swift`、`android/java/*`）・
//! 機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1187。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`live_activity_show_effect`] を画面
//! （`fandhe_core::screens::live_activity_show`）の effect として登録する。Activity 再生成では gallery の
//! `android_main` から [`reset_pending_tracking`] を呼ぶ。egui・`fandhe-device` には依存しない。
//!
//! iOS の Swift 薄層のリンク指定は `links` メタデータで最終成果物へ渡す（`build.rs`）。ウィジェット拡張
//! （`platforms/apple/FandheLiveActivity`）は `ios/fandhe_live_activity_attributes.swift` を共有する。
//!
//! 守る範囲と守らない範囲は [`LiveActivity`] の契約（`contract.rs`）を参照。

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
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::LiveActivityError;
