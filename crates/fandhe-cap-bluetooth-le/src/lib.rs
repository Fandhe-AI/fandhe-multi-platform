//! bluetooth-le（BLE スキャン: 近くの BLE 機器を一定時間探して、識別子・広告名・RSSI を一覧にする）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。macOS・iOS は CoreBluetooth、Windows は WinRT の `BluetoothLEAdvertisementWatcher`、
//! Android は `BluetoothLeScanner`（結果は 4 OS とも pull 型の `bridge` / `OBSERVED` で受ける）。Web は `fandhe-web-dom`。
//! `fandhe-gallery` の画面登録から effect `ble_scan_effect` が呼ばれる。終了時のスキャンの停止（[`stop_shared_scan`]）と
//! Activity 再生成時の保留の破棄（Android の [`reset_pending_tracking`]）は `fandhe-gallery` が呼ぶ。
//!
//! 守る範囲: 嘘の成功を出さないこと（0 台と失敗の区別。失敗時は部分結果を捨てる）、スキャンの確実な停止と資源の解放、
//! 機器名・識別子をログに出さないこと。守らない範囲: スキャン結果の網羅性と鮮度、権限ダイアログの回答待ちへの期限、複数プロセス。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_bluetooth_le()` は状態型に依存しないため、
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

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::BluetoothLeError;
