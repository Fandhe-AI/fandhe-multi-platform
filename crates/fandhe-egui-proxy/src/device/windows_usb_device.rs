//! Windows 向け USB 機器一覧実装のスタブ（Issue #505 が骨格を置き、#506 が埋める）。
//! `device/usb_device.rs::platform_usb_devices` の Windows 経路から呼ばれる。実装は常に
//! `Unsupported` を返し、ダミーの成功へは倒さない。
//!
//! #506 はこのファイルの `implementation()` だけを埋める。dispatch・画面・文言・
//! `device.rs` の `pub mod` 列は触らない。契約の SSOT は `fandhe_device::usb_device`。

use fandhe_device::usb_device::{UnsupportedUsbDevices, UsbDevices};

/// Windows の `UsbDevices` 実装を返す（#506 が置き換える）。
pub(crate) fn implementation() -> Box<dyn UsbDevices> {
    Box::new(UnsupportedUsbDevices)
}
