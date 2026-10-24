//! iOS 向け BLE の周辺機器役（Issue #779、親 #164 の b-ios）。
//!
//! 呼び出し元: `device/ble_peripheral.rs` の iOS 経路が [`implementation`] を呼ぶ。
//! 実体は macOS と共用の `apple_ble_peripheral::implementation`（CoreBluetooth `CBPeripheralManager` を
//! `apple/fandhe_apple_ble_peripheral.m` の C ABI 経由で呼ぶ）への委譲で、dispatch・共通コア画面・
//! trait 契約（`fandhe_device::ble_peripheral`）は変えない。
//!
//! iOS 固有の処理は `.m` の `TARGET_OS_IPHONE` 区画にある。
//!
//! - 背景遷移（`DidEnterBackground`）で `stopAdvertising`・`removeAllServices` を済ませてから「中断」を
//!   確定する（OS は背景でアドバタイズが止まったことを通知しないため。自動では再開しない。許可
//!   ダイアログ中の Inactive は中断にしない）
//! - 事象が届いた直後に、[`super::apple_ble_peripheral`] が `crate::ios::request_repaint` で次フレームを
//!   起こす（入力がないと egui が描画せず、画面が処理中のまま止まるため）
//!
//! # 守る範囲・守らない範囲
//!
//! 守る範囲は `apple_ble_peripheral` と同じ（嘘の成功の禁止、全終了経路での資源の解放、旧世代の通知を
//! 返さない）。守らない範囲は背景でのアドバタイズ（背景モードは宣言しない）、前景へ戻ったときの自動
//! 再開、複数プロセスでの同時公開。
//!
//! # 既知の天井
//!
//! シミュレータは CoreBluetooth の周辺機器役を持たず、非対応表示になる見込み。実機での公開・読み取り・
//! 背景遷移の挙動は実機確認（#776）で確かめる。

use std::sync::Arc;

use fandhe_device::ble_peripheral::BlePeripheral;

/// iOS の `BlePeripheral` 実装を返す（macOS と共用の Apple 実装へ委譲する）。
pub(crate) fn implementation() -> Arc<dyn BlePeripheral> {
    super::apple_ble_peripheral::implementation()
}
