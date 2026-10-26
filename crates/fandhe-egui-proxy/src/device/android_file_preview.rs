//! Android 向けファイル・動画のプレビューとサムネイル（Issue #917 の骨格。実体は b-android）。
//!
//! 呼び出し元: `device/file_preview.rs` の Android 経路が [`thumbnailer`]・[`present_preview`] を呼ぶ。
//! 今は `Unsupported` を返すだけで、dispatch・画面・文言・`android.rs` の reset 呼び出しは触らない。
//!
//! b-android への申し送り:
//! - `FilePreviewHelper.java` を足し、JNI 署名を Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - サムネイルは `ThumbnailUtils.createImageThumbnail(File, Size, CancellationSignal)`（API 29+）。
//!   API 29 未満は `Unsupported`。固定ファイルは Java 側の `getCacheDir()` に書く
//! - プレビューは FileProvider の content URI を `ACTION_VIEW` で他アプリへ渡し、`Ok(Some(HandedOff))` か
//!   `Ok(Some(NoHandler))` を返す。`compose_file_paths.xml` に 1 行足す（既存の FileProvider を共用）

use std::sync::Arc;

use fandhe_device::file_preview::{
    FileKind, FileThumbnailer, PreviewResult, UnsupportedFileThumbnailer,
};
use fandhe_device::DeviceError;

/// Android のサムネイル実装（今は非対応）。
pub(crate) fn thumbnailer() -> Arc<dyn FileThumbnailer> {
    Arc::new(UnsupportedFileThumbnailer)
}

/// Android のプレビュー提示（今は非対応）。
pub(crate) fn present_preview(
    _request_id: u64,
    _kind: FileKind,
    _bytes: Vec<u8>,
) -> Result<Option<PreviewResult>, DeviceError> {
    Err(DeviceError::Unsupported("file_preview"))
}
