//! 機能 crate「車載ディスプレイ」（car-display）。契約（`contract`）・失敗の種類（`error`）・effect ドライバ
//! （`driver`）・Android 実装（`android`＝Car App Library の `CarAppService`、`android/java/*`）・機能宣言
//! （`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1335。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`car_display_show_effect`] を画面
//! （`fandhe_core::screens::car_display_show`）の effect として登録する。egui・`fandhe-device` には依存しない。
//! Android 以外の OS は [`UnsupportedCarDisplay`] 固定（iOS の CarPlay は Apple の承認待ち）。
//!
//! 守る範囲と守らない範囲は [`CarDisplay`] の契約（`contract.rs`）と `android.rs` の doc を参照。

// unsafe を持たない（Android の JNI 呼び出しは共通層 `fandhe-platform-android` に閉じる）。
#![forbid(unsafe_code)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::CarDisplayError;
