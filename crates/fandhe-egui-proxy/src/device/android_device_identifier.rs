//! Android 向けアプリ・端末の識別子実装の骨格（Issue #903、親 #181）。`device/device_identifier.rs` の
//! Android 経路（`platform_device_identifier`）から呼ばれる。#904 が実体へ置き換える。触るのは本ファイルと
//! OS 固有の付帯物だけで、dispatch・fallback 行・`device.rs` の `pub mod` 列・共通コア画面・
//! `registry.rs` は触らない。実体が入るまでは `Unsupported` 固定で、成功には届かない。

use fandhe_device::device_identifier::{DeviceIdentifier, UnsupportedDeviceIdentifier};

/// 実装を返す（#904 で実体へ置き換える）。
pub(crate) fn implementation() -> Box<dyn DeviceIdentifier> {
    Box::new(UnsupportedDeviceIdentifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_never_succeeds() {
        assert!(implementation().read().is_err());
    }
}
