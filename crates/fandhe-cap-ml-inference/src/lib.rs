//! 機能 crate「機械学習モデル推論」（ml-inference）。契約（`contract`）・失敗の種類（`error`）・effect ドライバ
//! （`driver`）・OS 別実装（`android`＝NNAPI、`apple`＝Core ML の FFI、`ios`、`windows`＝WinML、
//! `apple/*.m`）・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1187。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`ml_infer_effect`] を画面
//! （`fandhe_core::screens::ml_infer`）の effect として登録する。Activity 再生成では Android の入口から
//! [`reset_pending_tracking`] を呼ぶ。egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は [`MlInference`] の契約（`contract.rs`）を参照。

// unsafe は Core ML の FFI（`apple`）と NNAPI の dlopen（`android`）だけ。
// それ以外の OS では禁止し、該当 OS では SAFETY コメント付きの最小範囲にだけ `allow` する。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::{platform_ml_inference, reset_pending_tracking};
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::MlInferenceError;
