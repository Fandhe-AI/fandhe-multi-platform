//! 機能 crate「歩数」（pedometer）。契約（`contract`）・effect ドライバ（`driver`）・OS 別実装
//! （`android`・`ios`・`windows`、`ios/*.m`、`android/java/*`）・機能宣言（`Cargo.toml` の
//! `[package.metadata.fandhe]`）を 1 つに集める（REQ-21 / TASK-21.4、Issue #1187。移行の手順は
//! `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`pedometer_read_effect`] を
//! 画面（`fandhe_core::screens::pedometer_read`）の effect として登録する。画面の状態型を参照するのは
//! `driver` だけで、入口 [`platform_pedometer`] は状態型に依存しない。egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は [`Pedometer`] の契約（`contract.rs`）を参照。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

mod contract;
mod driver;
// OS 別の実体。ホストのテストでも写像（純関数）を検査するため `test` でも載せる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
