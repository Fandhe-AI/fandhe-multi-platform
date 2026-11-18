//! 機能 crate「近くの端末との直接通信」（local-peer-connect。同じアプリを開いた近くの 2 台が固定文字列を 1 回ずつ送り合う）。
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 別実装（`apple`＝iOS・macOS の
//! MultipeerConnectivity、`ios`＝`apple` への委譲、`android`＝Wi-Fi Aware）・`apple/*.m`・`android/java/*`・機能宣言
//! （`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1335。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`peer_connect_effect`] を画面
//! （`fandhe_core::screens::peer_connect`）の effect として登録し、Activity 再生成時は `android_main` が
//! [`reset_pending_tracking`]（Android）を、アプリ終了時は終了フックが [`shutdown`] を呼ぶ。
//! 入口 [`platform_local_peer_connect`] は画面の状態型に依存しない。Windows は保留、Web は対象外のため `Unsupported` 固定。
//! egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は `contract.rs` のモジュール doc を参照。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
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

pub use contract::*;
#[cfg(target_os = "android")]
pub use driver::reset_pending_tracking;
pub use driver::{platform_local_peer_connect, poll_exchange, shutdown, start_exchange};
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::LocalPeerConnectError;
