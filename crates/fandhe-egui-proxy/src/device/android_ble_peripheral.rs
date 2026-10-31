//! Android 向け BLE 周辺機器役の実装の骨格（Issue #1122、親 #164）。
//! `device/ble_peripheral.rs` の Android 経路から呼ばれる。実体は #1123 が置き換える。
//!
//! 現状は [`UnsupportedBlePeripheral`] を返すだけ（成功を装うダミーにはしない）。
//! 契約は `fandhe_device::ble_peripheral` のモジュール doc に記載済み。
//! #1123 が埋める範囲は次のとおりで、dispatch・共通コア画面・`device.rs` の `pub mod`・trait doc は触らない。
//!
//! - `BlePeripheralHelper.java`（`HandlerThread` 1 本の直列キュー・`onStop` 中断・state の broadcast）
//! - JNI の受け口 `nativeOnBlePeripheralEvent(session, code)` と開始失敗コードの受け口
//! - `(session, OnEvent)` の static（`SessionSlot`）
//! - `with_activity` と `// SAFETY:`（`android_home_widget` と同形）
//! - manifest は宣言済み（#1120）
//! - 対応表 JSON の android セル

use std::sync::Arc;

use fandhe_device::ble_peripheral::{BlePeripheral, UnsupportedBlePeripheral};

/// Android の `BlePeripheral` 実装（骨格）。
pub(crate) fn implementation() -> Arc<dyn BlePeripheral> {
    Arc::new(UnsupportedBlePeripheral)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::ble_peripheral::StopOutcome;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_never_succeeds() {
        let p = implementation();
        assert!(matches!(
            p.start(1, Arc::new(|_| {})),
            Err(DeviceError::Unsupported("ble_peripheral"))
        ));
        assert!(matches!(p.stop(), Ok(StopOutcome::NotRunning)));
    }
}
