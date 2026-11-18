//! 機能 crate「ヘルスデータ」（health-data。今日の歩数を 1 回だけ期限付きで読む）。契約（`contract`）・
//! effect ドライバ（`driver`）・OS 別実装（`android`＝Health Connect、`ios`＝HealthKit の `.m`）・`ios/*.m`・
//! `android/java/*`・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1335。移行の手順は `docs/operations/capability-crate-migration.md`）。
//! 失敗の種類は契約の [`HealthDataFailure`] が持つため、独立の `error` モジュールは無い。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`health_steps_read_effect`] を画面
//! （`fandhe_core::screens::health_steps_read`）の effect として登録する。入口 [`platform_health_data`] は
//! 画面の状態型に依存しない。他 OS（Windows・macOS・Linux・Web）は `Unsupported` 固定。egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は `contract.rs` のモジュール doc と、各 OS 実装（`android.rs`・`ios.rs`）の doc を参照。

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
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::platform_health_data;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
