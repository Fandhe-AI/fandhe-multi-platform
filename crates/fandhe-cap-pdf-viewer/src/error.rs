//! pdf-viewer の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// pdf-viewer の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PdfViewerError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: pdf-viewer")]
    Unsupported,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: pdf-viewer")]
    InvalidInput,
    /// `DeviceError::Internal` に相当する。識別子（`contract` の `BROKEN` など、固定の `&'static str`）で
    /// 画面の失敗種別を分ける。OS の文言は持たず、表示文言にも出さない。
    #[error("内部エラーが発生しました: pdf-viewer")]
    Internal(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                PdfViewerError::Unsupported,
                "このプラットフォームでは未対応です: pdf-viewer",
            ),
            (PdfViewerError::InvalidInput, "入力が不正です: pdf-viewer"),
            (
                PdfViewerError::Internal("x"),
                "内部エラーが発生しました: pdf-viewer",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
