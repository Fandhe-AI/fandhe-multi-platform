//! alarm（アラーム・正確な時刻の予約: 固定時刻に 1 回だけ OS のアラームを鳴らす）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。iOS は AlarmKit（Swift 薄層を弱リンク）、Android は `AlarmManager.setAlarmClock`、
//! Windows は WinRT Toast（`scenario="alarm"`）。macOS・Linux・Web は対象外。`fandhe-gallery` の画面登録から effect
//! `alarm_show_effect` が呼ばれる。Windows の AUMID の登録は `fandhe-cap-notification` の Windows 実装を使う。
//!
//! 守る範囲: 予約の有無を OS の状態から読み直すこと、嘘の成功を出さないこと（許可の拒否・通知オフを成功にしない）、失敗経路での資源の解放。
//! 守らない範囲: 時刻の精度（OS の省電力の影響）、端末の再起動をまたぐ予約の維持、複数プロセス。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_alarm()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::AlarmError;
