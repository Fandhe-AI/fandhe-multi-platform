//! 機能 crate「ピクチャ・イン・ピクチャ」（picture-in-picture。映像の小窓表示）。契約（`contract`）・失敗の種類（`error`）・
//! effect ドライバ（`driver`）・OS 別実装（`apple`＝iOS・macOS の AVKit、`ios`＝`apple` を返す薄い層と `.m` からの再描画の受け口、
//! `android`＝Activity の小窓）・`apple/*.m`・`android/java/*`・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を
//! 1 つに集める（REQ-21 / TASK-21.4、Issue #1335。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`pip_show_effect`] を画面
//! （`fandhe_core::screens::pip_show`）の effect として登録し、Activity 再生成時は `android_main` が
//! [`reset_pending_tracking`]（Android）を、アプリ終了時は終了フックが [`shutdown`] を呼ぶ。
//! 入口 [`platform_picture_in_picture`] は画面の状態型に依存しない。Web の実装本体は `fandhe-web-dom` に残り、
//! この crate の契約の定数と純関数（`frame_color`・`FRAME_COLORS` など）だけを共有する（wasm32 でも通る）。
//! Windows は保留、Linux は `Unsupported` 固定。egui・`fandhe-device` には依存しない。
//!
//! 守る範囲と守らない範囲は `contract.rs` のモジュール doc を参照。

// unsafe は Apple の FFI・`.m` からの受け口（`apple`・`ios`）だけ。Android の JNI 呼び出しは共通層
// （`fandhe-platform-android`）に閉じるため、unsafe を持たない。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::{platform_picture_in_picture, reset_pending_tracking, shutdown};
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::PictureInPictureError;
