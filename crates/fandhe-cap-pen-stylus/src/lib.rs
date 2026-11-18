//! pen-stylus の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1334）。ペンの種類・筆圧・傾きを読む。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`pen_draw_effect`）と、Web の `fandhe-web-dom`（Pointer Events へ直接配線）。
//! 位置は描画層の `fandhe_runtime::HostView`（`canvas_pointer` / `canvas_pressed` / `canvas_released`）から、種類・筆圧・傾きは
//! OS 実装から取る（winit / egui-winit がそれらを渡さないため）。macOS・iOS・Android・Windows が実装し、他の OS は `Unsupported`。
//! Android は `PenStylusLifecycle` が共通層 `HostInput` へタッチの観測者を登録する（イベントは消費しない）。
//! 守る範囲は指・マウスの値を筆圧にしないこと・離したら値を出し続けないこと、守らない範囲は主ポインタ以外・履歴サンプル（`contract` の doc を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_pen_stylus()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    )),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    ),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::platform_pen_stylus;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::PenStylusError;
