//! 機能 crate「触覚フィードバック」（haptics）。契約（`contract`）・失敗の種類（`error`）・effect ドライバ
//! （`driver`）・OS 別実装（`android`＝`Vibrator`、`apple`＝iOS・macOS の ObjC、`windows`＝`VibrationDevice`、
//! `apple/*.m`、`android/java/*`）・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1187。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`haptics_tap_effect`] を画面
//! （`fandhe_core::screens::haptics_tap`）の effect として登録する。Web の実装は `fandhe-web-dom` に残る。
//! egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は [`Haptics`] の契約（`contract.rs`）を参照。

// unsafe は Apple の FFI（`apple`）だけ。それ以外の OS では禁止する。Android の JNI 呼び出しは
// 共通層（`fandhe-platform-android`）に閉じるため、unsafe を持たない。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::HapticsError;
