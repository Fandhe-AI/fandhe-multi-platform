//! ble-peripheral（BLE の周辺機器役: 固定の GATT サービスを広告し、読み取り専用の特性 1 つを公開する）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。macOS・iOS は CoreBluetooth の `CBPeripheralManager`（`apple`、iOS は `ios` から委譲）、
//! Windows は WinRT の GATT サーバー、Android は `BluetoothLeAdvertiser` と `BluetoothGattServer`。Web は対象外。
//! `fandhe-gallery` の画面登録から effect `ble_peripheral_effect` が呼ばれる。終了時の停止（[`shutdown`]）と Activity 再生成時の通知先の破棄
//! （Android の [`reset_pending_tracking`]）は `fandhe-gallery` が呼ぶ。
//!
//! 守る範囲: 公開の開始・停止を 1 本の順序にすること、嘘の成功を出さないこと、失敗経路を含む資源の解放（未解放は `stop` で回収し直せる）。
//! 守らない範囲: 接続した相手との通信内容、複数プロセス、OS 側の非同期確定との時間差。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_ble_peripheral()` は状態型に依存しないため、
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
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "android")]
pub use android::reset_pending_tracking;
pub use contract::*;
pub use driver::*;
pub use error::BlePeripheralError;
