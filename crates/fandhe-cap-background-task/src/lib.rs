//! background-task（OS のバックグラウンド実行を 1 件予約し、実行記録を読む）の機能 crate（REQ-21 / TASK-21.4、Issue #1332）。
//! Android は WorkManager、iOS は BGTaskScheduler、macOS は NSBackgroundActivityScheduler、Windows は Task Scheduler 2.0。
//! Web は `fandhe-web-dom` の `background_task.rs` が実装本体を持ち、本 crate の契約（`RunRecord`・`RECORD_KEY`・
//! `decode_record`）を参照する。呼び出し元は `fandhe-gallery` の画面登録（`background_task_effect`）と、起動時の入口
//! （Windows の [`parse_scheduled_args`] / [`run_scheduled`]、iOS の [`register`]）。実行記録の保存先は
//! `fandhe-cap-local-storage-kv` の `data_dir()`（Java の Worker が書く場所と一致させる）。
//!
//! 守る範囲: 保存失敗・読み出し失敗を「未実行」へ畳まないこと、二重予約は置き換えること、時刻の新しい記録を残すこと。
//! 守らない範囲: OS が実行時刻を保証しないこと、予約状態と OS 側の実際の状態の厳密な一致、複数プロセスの同時書き込みの調停。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_background_task()` は状態型に依存しないため、
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

#[cfg(target_os = "android")]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
mod ios;
#[cfg(target_os = "macos")]
mod macos;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::BackgroundTaskError;
#[cfg(target_os = "ios")]
pub use ios::register;
#[cfg(target_os = "windows")]
pub use windows::{parse_scheduled_args, run_scheduled};
