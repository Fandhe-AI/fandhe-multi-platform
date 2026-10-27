//! Windows 向け「無線の状態と切り替え」実装のスタブ（Issue #972、実装は #973）。
//! `device/radio_control.rs` の dispatch（Windows 経路）から呼ばれる。契約の SSOT は
//! `fandhe_device::radio_control`（「OS ごとの写像」節）。
//!
//! #973 への申し送り:
//! - `Radio::GetRadiosAsync` を Kind で絞った先頭 1 つを使う。無ければ `NotPresent`
//! - `RequestAccessAsync` → `SetStateAsync`。`Allowed` は `Accepted`、`DeniedByUser` は
//!   `PermissionDenied`、`DeniedBySystem` / `Unspecified` は `DENIED_BY_SYSTEM`
//! - `windows` クレートの `Devices_Radios` は導入済み。`unsafe` と `RoInitialize` は使わない
//! - `windows_bluetooth_le.rs` の `RadioState` の写像を参照する
//!
//! #973 が cfg を変えずに済むよう `any(windows, test)`。

use std::sync::Arc;

use fandhe_device::radio_control::{RadioControl, UnsupportedRadioControl};

/// Windows の `RadioControl` 実装を返す。#973 までは `Unsupported` 固定（ダミーの成功へは倒さない）。
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
