//! Android 向けスキャナー・カメラ機器からの取り込み（骨格は #981 (a)、実装は #983）。
//! `device/scanner_devices.rs::platform` の Android 経路から呼ばれる。現状は `Unsupported` を返すだけで、
//! ダミーの成功へは倒さない。
//!
//! # #983 への申し送り
//!
//! - `UsbDeviceHelper.java`・`RequestTracker.java`・JNI の 3 か所の署名を一致させる
//! - 許可の broadcast を受けたら extras を信じず `hasPermission` を読み直す。`PendingIntent` は
//!   `FLAG_IMMUTABLE` + `setPackage`、API 33 以降は `RECEIVER_NOT_EXPORTED`
//! - 許可ダイアログの `onPause` は中断にしない。ダイアログを閉じた操作は `PermissionDenied`
//! - `ACTION_USB_DEVICE_DETACHED` は `Interrupted`
//! - `MtpDevice.close` と `UsbDeviceConnection.close` を成功・失敗・期限切れの全経路で行う
//! - マニフェストの権限宣言は足さない
//! - 写像の純関数 `decode` はホストの `cargo test` で検査できるよう cfg を付けない
//! - dispatch・`device.rs` の `pub mod` 列・共通コア画面は触らない。Activity 再生成の
//!   `reset_pending_tracking` 配線は #981 (a) で済んでいる

use std::sync::Arc;

use fandhe_device::scanner_devices::{ScannerDevices, UnsupportedScannerDevices};

/// Android の `ScannerDevices` 実装を返す（#983 まで `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn ScannerDevices> {
    Arc::new(UnsupportedScannerDevices)
}
