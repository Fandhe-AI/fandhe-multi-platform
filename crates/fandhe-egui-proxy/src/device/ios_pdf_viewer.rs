//! iOS 向け PDF 表示実装（Issue #471。親 #107 の b-ios）。
//!
//! 呼び出し元: `device/pdf_viewer.rs` の iOS 経路が `implementation` を呼ぶ。macOS（#469）と共用の
//! `apple_pdf_viewer::ApplePdfPageRenderer`（CoreGraphics。`.m` に `#if` 分岐は無い）を返すだけで、
//! dispatch・画面・文言は触らない。
//!
//! 契約表の点検:
//! - 再描画（4）: 結果はワーカーから `CompletionSender::send` で届き、`send` が同じ `egui::Context` の
//!   `request_repaint()` を呼ぶ。OS のコールバックは無いので `crate::ios::request_repaint` の追加呼び出しは要らない。
//! - ライフサイクル（1）: 背景遷移で中断しない。suspend をはさんで期限を過ぎたらドライバが Timeout で確定し、
//!   遅れた結果は世代照合で捨てる。
//! - FFI（5）: ワーカーから呼ぶ同期呼び出しで、UI も main queue も要らない。

use std::sync::Arc;

use fandhe_device::pdf_viewer::PdfPageRenderer;

/// iOS の `PdfPageRenderer` 実装（macOS と共用の CoreGraphics 実装）を返す。
pub(crate) fn implementation() -> Arc<dyn PdfPageRenderer> {
    Arc::new(super::apple_pdf_viewer::ApplePdfPageRenderer)
}
