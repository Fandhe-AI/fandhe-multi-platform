//! Windows 向けスキャナー取り込み（骨格は #981 (a)、実装は #984）。
//! `device/scanner_devices.rs::platform` の Windows 経路から呼ばれる。現状は `Unsupported` を返すだけで、
//! ダミーの成功へは倒さない。
//!
//! # #984 への申し送り
//!
//! - `windows` クレートの feature `Devices_Scanners` は #984 が `Cargo.toml` に足す
//! - 取り込み元は Flatbed → Feeder の順、形式は JPEG → PNG の順で選ぶ
//! - `ScanPreviewToStreamAsync` の結果は `Succeeded` を確認する。メモリのストリームへ取り込み、
//!   Pictures Library へ保存しない
//! - HRESULT の写像は純関数にしてテストする。握りつぶして `-1` 等へ潰さない
//! - 期限切れは `Cancel()` を呼び、解除し終えてから `TimedOut` を返す
//! - dispatch・`device.rs` の `pub mod` 列・共通コア画面は触らない

use std::sync::Arc;

use fandhe_device::scanner_devices::{ScannerDevices, UnsupportedScannerDevices};

/// Windows の `ScannerDevices` 実装を返す（#984 まで `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn ScannerDevices> {
    Arc::new(UnsupportedScannerDevices)
}
