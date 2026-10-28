//! iOS 向けスキャナー・カメラ機器からの取り込み（骨格は #981 (a)、実装は #982）。
//! `device/scanner_devices.rs::platform` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_scanner_devices::AppleScannerDevices`（`apple/fandhe_apple_scanner_devices.m`、
//! ImageCaptureCore）へ委譲する薄いラッパーを返す。iOS 固有の動き（取り込みの許可
//! `requestContentsAuthorization`、背景遷移での `Interrupted`）は `.m` の `TARGET_OS_IOS` 区画が持つ。
//!
//! 契約表 4（再描画）: 結果は `RequestSlot::deliver` で置く pull 型。委譲から戻った直後に
//! `crate::ios::request_repaint` で次フレームを起こす。この呼び出しは `deliver` より前に走るが、
//! 保留中は `poll_and_finish` が `request_repaint_after` で次の確認を予約するので結果は拾われる
//! （geocoding #692 と同じ形）。守る範囲・守らない範囲は `fandhe_device::scanner_devices` の doc を正とする。

use std::sync::Arc;
use std::time::Duration;

use fandhe_device::scanner_devices::{ImportedImage, ScannerDevices, ScannerImportFailure};

use super::apple_scanner_devices::AppleScannerDevices;

/// `AppleScannerDevices` へ委譲し、戻った直後に次フレームを起こす iOS 用ラッパー。
/// 戻り値・失敗の写像は委譲先と同一。
struct IosScannerDevices;

impl ScannerDevices for IosScannerDevices {
    fn import_one(&self, remaining: Duration) -> Result<ImportedImage, ScannerImportFailure> {
        let result = AppleScannerDevices.import_one(remaining);
        crate::ios::request_repaint();
        result
    }
}

/// iOS の `ScannerDevices` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn ScannerDevices> {
    Arc::new(IosScannerDevices)
}
