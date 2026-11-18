//! eye-dropper（画面上の 1 点の色を拾う）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1191・#1359）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`eye_dropper_pick_effect`）と、画面の状態型を持たない
//! 外のアプリ（[`begin_eye_dropper`] / [`poll_eye_dropper`] / [`discard_eye_dropper`]）。契約（`PickedColor`・結果コードの写像）は
//! `fandhe_device::eye_dropper` に残る（color-picker が未移行で共有するため）。
//! macOS（`NSColorSampler`、`macos/fandhe_macos_eye_dropper.m`）と Web（`web.rs`、`new EyeDropper().open()`。#1359 で
//! `fandhe-web-dom` から移した）が実装し、結果は非同期に `RequestSlot` の受け口へ届く。他の OS は `Unsupported`
//! （画面にも公開入口にも出し、成功にしない）。同期の [`platform_eye_dropper`] は常に `Unsupported` のまま。
//!
//! # 画面 effect と公開入口を同時に使うとき
//!
//! 両者は同じ slot を共有する。片方が進行中のあいだ、後から来た方は `Internal("eye_dropper busy")` で断る。
//! 公開入口の結果が未取得のあいだも、画面側の開始は同じく断る（画面は処理中の失敗を表示する）。[`discard_eye_dropper`] は Web なら中断し、macOS ではサンプラーを閉じる API が無いため
//! 利用者が選ぶか Esc を押すまで次の開始を断る。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求の順序・古い世代の通知の破棄・失敗を成功と見せないこと・複数の入口からの同時起動を busy で断ること。
//! 守らない: 拾った色の保持（ログにも出さない）、複数タブ・複数プロセスでの同時操作。
//!
//! effect ドライバ（`driver`）・OS 実装を 1 つに集める。画面の状態型を参照してよいのは `driver` だけ（`web` は画面に依存しない口だけを持つ）。

// unsafe は macOS の FFI（`.m` の C ABI と受け口）に限る（`// SAFETY:` を付ける）。macOS だけ deny へ緩める。
#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]
#![cfg_attr(target_os = "macos", deny(unsafe_code))]

/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_arch = "wasm32")]
mod web;
// ブラウザ API に触れない写像。wasm32 以外ではテストでだけ使う。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod web_result;

pub use driver::begin_eye_dropper;
pub use driver::{discard_eye_dropper, platform_eye_dropper, poll_eye_dropper};
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::EyeDropperError;
// 契約の正本は fandhe-device に残し、利用者が直接依存しなくて済むよう再公開する。
pub use fandhe_device::eye_dropper::{EyeDropper, PickedColor, UnsupportedEyeDropper};
