//! Windows 向け PDF 表示のスタブ（Issue #469。親 #107 の windows の子が `Windows.Data.Pdf` で埋める）。
//!
//! 呼び出し元: `device/pdf_viewer.rs` の Windows 経路が `implementation` を呼ぶ。windows の子は本ファイルの
//! 中身（と feature `Data_Pdf` の追加）だけを埋める。dispatch・画面・文言は触らない。
//! それまでは `Unsupported` を返し、ダミーの成功は返さない。

use std::sync::Arc;

use fandhe_device::pdf_viewer::{PdfPageRenderer, UnsupportedPdfPageRenderer};

/// Windows の `PdfPageRenderer` 実装を返す（現状は非対応）。
pub(crate) fn implementation() -> Arc<dyn PdfPageRenderer> {
    Arc::new(UnsupportedPdfPageRenderer)
}
