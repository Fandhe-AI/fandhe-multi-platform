//! Android 向けライト（トーチ）実装のスタブ（Issue #458、親 #106。b-android の #461 が中身を埋める）。
//! `device/flashlight.rs` の dispatch（Android 経路）から呼ばれる。契約は `fandhe_device::flashlight`。
//!
//! dispatch・画面・文言は触らず、本ファイルの [`implementation`] だけを置き換える。
//! 埋めるまでは常に `Unsupported` を返し、対応表のセルは `possible` のまま。

use std::sync::Arc;

use fandhe_device::flashlight::{Flashlight, UnsupportedFlashlight};

/// Android の実装を返す。現状はスタブ（常に `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn Flashlight> {
    Arc::new(UnsupportedFlashlight)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_never_success() {
        let f = implementation();
        assert!(matches!(f.set_on(true), Err(DeviceError::Unsupported(_))));
        assert!(matches!(f.set_on(false), Err(DeviceError::Unsupported(_))));
        assert!(matches!(f.is_on(), Err(DeviceError::Unsupported(_))));
    }
}
