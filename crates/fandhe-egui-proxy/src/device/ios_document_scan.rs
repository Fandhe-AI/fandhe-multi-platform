//! iOS 向け書類スキャンのスタブ（Issue #747、親 #145）。`device/document_scan.rs` の iOS 経路が
//! [`implementation`] を呼ぶ。実体（#748）が入るまでは `Unsupported` を返し、成功を返さない。
//!
//! # 後続 #748 が埋める範囲
//!
//! - `.m` で `VNDocumentCameraViewController` を main queue で提示する
//! - `isSupported` の判定と `AVCaptureDevice` のカメラ権限判定
//! - delegate の 3 経路（完了・キャンセル・失敗）すべてで撮影 UI を閉じる
//! - `UIImage` の JPEG 化（`pageCount` を確かめてから `imageOfPageAtIndex:`）
//! - 表示直前の `try_begin_present`、結果の `complete_from_os` の呼び出し順（契約は
//!   `fandhe_device::document_scan`）
//! - 結果を入れた直後の `crate::ios::request_repaint`
//! - `NSCameraUsageDescription` の文言に機能名を足す
//! - `build.rs` への `.m` の登録
//!
//! dispatch・共通コア画面・fallback 行・`device.rs` の `pub mod` 列は #747 の最終形のまま触らない。

use std::sync::Arc;

use fandhe_device::document_scan::{DocumentScanner, UnsupportedDocumentScanner};

/// 現在の iOS 実装（スタブ。常に `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn DocumentScanner> {
    Arc::new(UnsupportedDocumentScanner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::document_scan::DocumentScanFailure;

    #[test]
    fn stub_never_succeeds() {
        assert_eq!(
            implementation().scan(1),
            Err(DocumentScanFailure::Unsupported)
        );
    }
}
