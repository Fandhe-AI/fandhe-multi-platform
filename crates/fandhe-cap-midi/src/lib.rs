//! midi（MIDI 入力の受信とノート表示） の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 呼び出し元は `fandhe-gallery` の画面登録（`midi_show_effect`）、Activity 再生成時に `reset_pending_tracking` を呼ぶアプリの `android_main`、
//! 終了時に購読を止める `fandhe-egui-proxy::device::shutdown`（`shutdown`。過渡期の依存で #1194 で消える）。契約のパーサ
//! （`MidiParser`・`NoteEvent`）は Web 実装（`fandhe-web-dom`、本体は web-dom に残る）と共有する。Android は Activity ごとの
//! helper（`MidiLifecycle`）で購読と HandlerThread を持つ。守る範囲は購読の解放と、停止後に届く古い世代の入力を捨てることで、
//! 背景での受信継続・出力・デバイス名は守らない。演奏内容はログへ出さない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_midi()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::MidiError;
