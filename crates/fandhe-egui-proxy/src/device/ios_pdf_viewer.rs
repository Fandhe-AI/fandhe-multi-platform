//! iOS 向け PDF 表示のスタブ（Issue #469。親 #107 の b-ios が埋める）。
//!
//! 呼び出し元: `device/pdf_viewer.rs` の iOS 経路が `implementation` を呼ぶ。b-ios は本ファイルの
//! 中身だけを、macOS と共用の `apple_pdf_viewer::ApplePdfPageRenderer` を返す形へ差し替える。
//! dispatch・画面・文言は触らない。それまでは `Unsupported` を返し、ダミーの成功は返さない。

use std::sync::Arc;

use fandhe_device::pdf_viewer::{PdfPageRenderer, UnsupportedPdfPageRenderer};

/// iOS の `PdfPageRenderer` 実装を返す（現状は非対応）。
pub(crate) fn implementation() -> Arc<dyn PdfPageRenderer> {
    Arc::new(UnsupportedPdfPageRenderer)
}
