//! Android 向け PDF 表示のスタブ（Issue #469。親 #107 の b-android が `PdfRenderer` で埋める）。
//!
//! 呼び出し元: `device/pdf_viewer.rs` の Android 経路が `implementation` を呼ぶ。b-android は本ファイルの
//! 中身（と Java ヘルパー・JNI）だけを埋める。dispatch・画面・文言は触らない。
//! それまでは `Unsupported` を返し、ダミーの成功は返さない。

use std::sync::Arc;

use fandhe_device::pdf_viewer::{PdfPageRenderer, UnsupportedPdfPageRenderer};

/// Android の `PdfPageRenderer` 実装を返す（現状は非対応）。
pub(crate) fn implementation() -> Arc<dyn PdfPageRenderer> {
    Arc::new(UnsupportedPdfPageRenderer)
}
