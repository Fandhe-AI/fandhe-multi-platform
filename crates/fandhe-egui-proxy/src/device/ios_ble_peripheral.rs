//! iOS 向け BLE の周辺機器役（Issue #778 の (a) が置くスタブ。親 #164、b-ios は #779）。
//!
//! 呼び出し元: `device/ble_peripheral.rs` の iOS 経路が `implementation` を呼ぶ。現在は
//! `UnsupportedBlePeripheral`（非対応表示）を返すだけで、iOS では何も公開しない。
//!
//! b-ios（#779）の作業範囲は次のとおりで、本ファイル以外の dispatch・画面・`device.rs`・
//! `build.rs` は変えない。
//! - 本ファイルを `apple_ble_peripheral::implementation()` への委譲へ差し替える
//! - `apple/fandhe_apple_ble_peripheral.m` の `TARGET_OS_IPHONE` 区画に背景遷移
//!   （`UIApplicationDidEnterBackgroundNotification`）を埋める。購読したら専用キューで止め、
//!   コード「中断」を送る
//! - iOS の `Info.plist` の利用目的の文言を足す
//! - 値が確定した直後に `crate::ios::request_repaint` を呼ぶ（入力が無いと egui が描画せず
//!   画面が処理中のまま止まるため。契約表 4）
//!
//! 契約の SSOT は `fandhe_device::ble_peripheral`。

use std::sync::Arc;

use fandhe_device::ble_peripheral::{BlePeripheral, UnsupportedBlePeripheral};

/// iOS 経路の実装（b-ios が委譲へ差し替えるまでは非対応）。
pub(crate) fn implementation() -> Arc<dyn BlePeripheral> {
    Arc::new(UnsupportedBlePeripheral)
}
