//! sleep-lock-events の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。スリープ・画面の消灯と点灯・ロックと解除の通知を OS から受け取る。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`sleep_lock_events_show_effect`）。契約の純関数・型（`EventLog`・`LockState`・`SleepLockEvent` 等）は
//! Web 実装（`fandhe-web-dom`）も使うため、機能 crate は wasm32 でも通るよう OS 別の依存を target で外す。
//! 守る範囲は 1 プロセス内の受信順・失敗を成功へ畳まないこと・全終了経路での購読解除、守らない範囲はシステムが溜めた通知の順序と
//! 画面を開く前・止めた後の出来事（各 OS 実装の doc を参照）。macOS・Windows・Android が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_sleep_lock_events()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::SleepLockEventsError;
