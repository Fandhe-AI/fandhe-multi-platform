//! Windows 向けファイル・動画のプレビューとサムネイル（Issue #917 の骨格。実体は windows の子）。
//!
//! 呼び出し元: `device/file_preview.rs` の Windows 経路が [`thumbnailer`]・[`present_preview`] を呼ぶ。
//! 今は `Unsupported` を返すだけで、dispatch・画面・文言は触らない。
//!
//! windows の子への申し送り:
//! - [`thumbnailer`] を `SHCreateItemFromParsingName` → `IShellItemImageFactory::GetImage`
//!   （`SIIGBF_THUMBNAILONLY`。アイコンへ倒れない）の実装へ差し替える。`GetDIBits` で RGBA にし、
//!   返る `HBITMAP` は `DeleteObject` する。UI スレッドでは呼ばない。固定ファイルは
//!   `fandhe_device::file_preview::write_fixed` で書く。HRESULT は全呼び出し箇所で一貫して写像する
//! - [`present_preview`] は `Ok(Some(PreviewResult::TargetUnavailable))` を返す形へ差し替える
//!   （Windows はプレビュー表示の対象外）

use std::sync::Arc;

use fandhe_device::file_preview::{
    FileKind, FileThumbnailer, PreviewResult, UnsupportedFileThumbnailer,
};
use fandhe_device::DeviceError;

/// Windows のサムネイル実装（今は非対応）。
pub(crate) fn thumbnailer() -> Arc<dyn FileThumbnailer> {
    Arc::new(UnsupportedFileThumbnailer)
}

/// Windows のプレビュー提示（今は非対応）。
pub(crate) fn present_preview(
    _request_id: u64,
    _kind: FileKind,
    _bytes: Vec<u8>,
) -> Result<Option<PreviewResult>, DeviceError> {
    Err(DeviceError::Unsupported("file_preview"))
}
