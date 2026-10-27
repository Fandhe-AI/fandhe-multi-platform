//! Android 向け「無線の状態と切り替え」実装のスタブ（Issue #972、実装は #974）。
//! `device/radio_control.rs` の dispatch（Android 経路）から呼ばれる。契約の SSOT は
//! `fandhe_device::radio_control`（「OS ごとの写像」節）。
//!
//! #974 への申し送り:
//! - Activity の結果は `RequestTracker`（`RequestTracker.java`）で追跡し、Bluetooth のオンは
//!   `ACTION_REQUEST_ENABLE`。`RESULT_OK` は `Applied`、`RESULT_CANCELED` は `NotChanged`
//! - Wi-Fi の切り替えと Bluetooth のオフは設定画面を開いて `Opened`。設定パネルは API 29 で分岐する
//! - `BLUETOOTH_CONNECT` と `ACCESS_WIFI_STATE` は宣言済み（権限の追加なし）
//! - JNI の署名は Java 側・スタブ・実体の 3 か所で一致させる
//! - 下敷きは `SystemSettingsHelper.java`・`BluetoothClassicHelper.java`・`WifiInfoHelper.java`
//!
//! #974 が cfg を変えずに済むよう `any(android, test)`。

use std::sync::Arc;

use fandhe_device::radio_control::{RadioControl, UnsupportedRadioControl};

/// Android の `RadioControl` 実装を返す。#974 までは `Unsupported` 固定（ダミーの成功へは倒さない）。
pub(crate) fn implementation() -> Arc<dyn RadioControl> {
    Arc::new(UnsupportedRadioControl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::radio_control::RadioKind;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_unsupported() {
        assert!(matches!(
            implementation().read(RadioKind::Wifi),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            implementation().set(RadioKind::Bluetooth, true),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
