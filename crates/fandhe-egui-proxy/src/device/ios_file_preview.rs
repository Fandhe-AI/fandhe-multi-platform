//! iOS 向けファイル・動画のプレビューとサムネイル（Issue #917 の骨格。実体は b-ios）。
//!
//! 呼び出し元: `device/file_preview.rs` の iOS 経路が [`thumbnailer`]・[`present_preview`] を呼ぶ。
//! 今は `Unsupported` を返すだけで、dispatch・画面・文言は触らない。
//!
//! b-ios への申し送り:
//! - [`thumbnailer`] を `apple_file_preview::AppleFileThumbnailer`（macOS と共用の QuickLookThumbnailing）
//!   を返す形へ差し替える。`build.rs` のサムネイル部分は iOS 分岐から既に呼ばれている
//! - [`present_preview`] は `QLPreviewController` を提示し、閉じた通知（`previewControllerDidDismiss`）の
//!   直後に `crate::ios::request_repaint` を呼ぶ（契約表 4）。`QuickLook` のリンクと `.m` は b-ios が足す

use std::sync::Arc;

use fandhe_device::file_preview::{
    FileKind, FileThumbnailer, PreviewResult, UnsupportedFileThumbnailer,
};
use fandhe_device::DeviceError;

/// iOS のサムネイル実装（今は非対応）。
pub(crate) fn thumbnailer() -> Arc<dyn FileThumbnailer> {
    Arc::new(UnsupportedFileThumbnailer)
}

/// iOS のプレビュー提示（今は非対応）。
pub(crate) fn present_preview(
    _request_id: u64,
    _kind: FileKind,
    _bytes: Vec<u8>,
) -> Result<Option<PreviewResult>, DeviceError> {
    Err(DeviceError::Unsupported("file_preview"))
}
