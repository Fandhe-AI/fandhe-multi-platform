//! Android 向け書類スキャンの骨格（親 #145、Issue #1124）。`device/document_scan.rs` の Android 経路
//! （dispatch）から [`implementation`] が呼ばれる。実体（ML Kit Document Scanner）は #1125 が埋める。
//! 契約の SSOT は `fandhe_device::document_scan`。
//!
//! 現状は常に `Unsupported` を返す。成功やキャンセルを偽って返さない（契約表 6）。
//!
//! # #1125 が埋める範囲（dispatch・trait doc・共通コア画面・`device.rs` の `pub mod` 列は触らない）
//!
//! - 本ファイル: `DocumentScanner::scan` の実体（JNI で Java を呼ぶ）と、`try_begin_present` /
//!   `complete_from_os` を呼ぶ JNI の受け口 `nativeTryBeginPresent` / `nativeOnDocumentScanResult`
//! - Java: `GmsAvailability.java`・`DocumentScanHelper.java`、`MainActivity` のライフサイクル連携。
//!   JNI の署名は Java 側・no-op スタブ・本ファイルの 3 か所で一致させる

use std::sync::Arc;

use fandhe_device::document_scan::{DocumentScanner, UnsupportedDocumentScanner};

/// Android 用の実装を返す。#1124 時点では `Unsupported` 固定のスタブ。
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
