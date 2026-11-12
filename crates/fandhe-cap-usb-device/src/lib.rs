//! 機能 crate「USB 機器の一覧」（usb-device）。契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・
//! OS 別実装（`android`・`macos`・`windows`）・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1189。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`usb_device_show_effect`] を画面
//! （`fandhe_core::screens::usb_device_show` など）の effect として登録する。
//! 画面の状態型を参照してよいのは `driver` だけ。契約と OS 実装は状態型に依存しない。
//!
//! 守る範囲と守らない範囲は `contract.rs` の契約を参照。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::UsbDeviceError;
