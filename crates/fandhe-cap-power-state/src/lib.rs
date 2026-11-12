//! 機能 crate「電源状態」（power-state）。契約（`contract`）・失敗の種類（`error`）・effect ドライバ
//! （`driver`）・OS 別実装（`android`・`ios`・`macos`・`windows`、`ios/*.m`・`macos/*.m`、
//! `android/java/*`）・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1187。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`power_state_show_effect`] を画面
//! （`fandhe_core::screens::power_state_show`）の effect として登録する。Web の実装は `fandhe-web-dom`
//! に残り、OS に触れない純関数（[`percent_from_raw`]）だけを本 crate の契約から使う。
//! egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は [`PowerState`] の契約（`contract.rs`）を参照。

// unsafe は OS 実装の FFI・JNI 呼び出しに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
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
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::PowerStateError;
