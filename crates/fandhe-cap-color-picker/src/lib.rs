//! color-picker（OS の色の選択ダイアログ）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`color_picker_pick_effect`）と、契約の純関数（`color_from_input_value`・
//! `INITIAL_RGB`）と `ColorPickerError` を共有する `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。
//! iOS（`UIColorPickerViewController`）・macOS（`NSColorPanel`）・Windows（`ChooseColorW`）が実装し、他の OS は `Unsupported`。
//! 色の型と検証はスポイト（eye-dropper）と共有するため `fandhe-device` に残してある。
//! 守る範囲は範囲外の値・変換失敗・キャンセルを成功色に畳まないこと。守らない範囲は表示中の他機能の操作（Windows は
//! `ChooseColorW` が自前のメッセージループを回すため、表示中は描画が止まる）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_color_picker()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "ios", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::ColorPickerError;
