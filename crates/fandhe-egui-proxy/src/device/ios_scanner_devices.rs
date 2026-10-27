//! iOS 向けスキャナー・カメラ機器からの取り込み（骨格は #981 (a)、実装は #982）。
//! `device/scanner_devices.rs::platform` の iOS 経路から呼ばれる。現状は `Unsupported` を返すだけで、
//! ダミーの成功へは倒さない。
//!
//! # #982 への申し送り
//!
//! - `implementation()` が macOS と共用の `AppleScannerDevices` を返すよう差し替える
//!   （戻り直後に `crate::ios::request_repaint` を呼ぶ薄いラッパーでもよい。pull 型なので必須ではない。
//!   geocoding #692 の前例）
//! - `apple/fandhe_apple_scanner_devices.m` の `#if TARGET_OS_IOS` 区画に
//!   `requestContentsAuthorization` / `contentsAuthorizationStatus` を足す（denied・restricted は
//!   戻りコード 5 = `PermissionDenied`）
//! - 背景へ遷移したら `Interrupted`
//! - `NSCameraUsageDescription` は既存キーの文言に用途を足すだけで、新しいキーは足さない
//! - `dispatch`・`device.rs` の `pub mod` 列・`build.rs`・共通コア画面は触らない

use std::sync::Arc;

use fandhe_device::scanner_devices::{ScannerDevices, UnsupportedScannerDevices};

/// iOS の `ScannerDevices` 実装を返す（#982 まで `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn ScannerDevices> {
    Arc::new(UnsupportedScannerDevices)
}
