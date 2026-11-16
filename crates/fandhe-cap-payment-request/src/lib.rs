//! 機能 crate「物・サービスの決済」（payment-request。Google Pay の TEST 環境で固定 1 円の決済シートを出す。実決済はしない）。
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・Android 実装（`android`＝Google Pay API、
//! `android/java/*`）・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1335。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`payment_request_effect`] を画面
//! （`fandhe_core::screens::payment_request`）の effect として登録し、Activity 再生成時は `android_main` が
//! [`reset_pending_tracking`]（Android）を呼ぶ。Web の実装本体は `fandhe-web-dom` に残り、この crate の
//! 契約の定数と写像（`METHOD_DATA_JSON`・`outcome_from_dom_error` など）だけを共有する（wasm32 でも通る）。
//! iOS・macOS は対応表で保留、Windows は非対応のため `Unsupported` 固定。egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は `contract.rs` のモジュール doc を参照。

// unsafe は Android の JNI エクスポート（`android`）だけ。それ以外の OS では禁止する。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::PaymentRequestError;
