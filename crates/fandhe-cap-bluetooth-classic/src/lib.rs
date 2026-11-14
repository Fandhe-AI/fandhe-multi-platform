//! bluetooth-classic（Bluetooth Classic: ペアリング済み機器の一覧と、近くの機器の検索）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。macOS は `IOBluetooth`、Windows は `BluetoothFindFirstDevice`、Android は
//! `BluetoothAdapter` の検索（結果は pull 型 `bridge` で受ける）。iOS・Web は対象外。`fandhe-gallery` の画面登録から effect
//! `bluetooth_classic_show_effect` が呼ばれる。終了時の検索の停止（[`stop_shared_discovery`]）は `fandhe-gallery` の終了処理が呼ぶ。
//!
//! 守る範囲: 嘘の成功を出さないこと（0 件と失敗の区別）、検索の確実な停止と資源の解放、機器名・アドレスをログに出さないこと。
//! 守らない範囲: 検索結果の網羅性と鮮度、複数プロセス、OS 側の非同期確定との時間差。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_bluetooth_classic()` は状態型に依存しないため、
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
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::BluetoothClassicError;
