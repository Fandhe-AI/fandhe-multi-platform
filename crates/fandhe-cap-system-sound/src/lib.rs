//! system-sound（OS 標準の効果音・ビープ）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`system_sound_beep_effect`）。Android（`ToneGenerator`）・
//! macOS（`NSBeep`）・Windows（`PlaySoundW`）が実装し、iOS・Linux は `Unsupported`。Android の `ToneGenerator` は
//! Activity ごとの helper が持ち、`SystemSoundLifecycle`（非公開 provider）が生成・破棄を知らせる。
//! 守る範囲は `NotPlayed`・失敗を成功に畳まないことと、破棄後に資源を作らないこと。
//! 守らない範囲は実際に聞こえたか（消音・音量 0）・連打時の置き換え・背景中の押下。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_system_sound()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は macOS・Windows の FFI に限る（Android の JNI は共通層の `with_activity_class` が持つ）（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "macos", target_os = "windows"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::SystemSoundError;
