//! memory-pressure（メモリ逼迫・負荷の通知）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`memory_pressure_show_effect`）、終了時の
//! `fandhe-gallery`（`shutdown`）、契約の純関数と型（`PressureEvent`・`EventLog`・`ALL_EVENTS`）を共有する
//! `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。Android（`ComponentCallbacks2`）・iOS・macOS（OS の通知）・
//! Windows（`CreateMemoryResourceNotification`）が実装する。購読は id で照合し、停止後・再生成前の古い id の通知は捨てる。
//! 守る範囲は 1 プロセス内の受信順・失敗と非対応を成功に畳まないこと・全終了経路での登録解除。
//! 守らない範囲は画面を開く前・止めた後・背景で停止中の合図、OS 間の「逼迫」基準の差。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_memory_pressure()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポート（`nativeOn*`）と、Apple の `.m` から呼ばれる受け口（`driver.rs`）に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
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
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::MemoryPressureError;
