//! eye-dropper（画面上の 1 点の色を拾う）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1191）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`eye_dropper_pick_effect`）。契約（`PickedColor`・結果コードの写像）は
//! `fandhe_device::eye_dropper` に残る（color-picker が未移行で共有するため。Web 実装本体も `fandhe-web-dom`）。
//! macOS（`NSColorSampler`、`macos/fandhe_macos_eye_dropper.m`）が実装し、結果は非同期に `RequestSlot` の受け口へ届く。
//! 他の OS は `Unsupported` を画面へ出す。守る範囲は 1 プロセス内の要求の順序・古い世代の通知の破棄・失敗を成功と見せないこと、
//! 守らない範囲は拾った色の保持（ログにも出さない）・複数画面からの同時起動。
//!
//! effect ドライバ（`driver`）・OS 実装を 1 つに集める。画面の状態型を参照してよいのは `driver` だけ。

// unsafe は macOS の FFI（`.m` の C ABI と受け口）に限る（`// SAFETY:` を付ける）。macOS だけ deny へ緩める。
#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]
#![cfg_attr(target_os = "macos", deny(unsafe_code))]

mod driver;
#[cfg(target_os = "macos")]
mod macos;

pub use driver::*;
// 契約の正本は fandhe-device に残し、利用者が直接依存しなくて済むよう再公開する。
pub use fandhe_device::eye_dropper::{EyeDropper, PickedColor, UnsupportedEyeDropper};
