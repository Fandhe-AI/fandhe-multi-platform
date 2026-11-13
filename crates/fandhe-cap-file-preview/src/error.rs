//! file-preview の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// file-preview の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FilePreviewError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: file-preview")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Cancelled` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                FilePreviewError::Unsupported,
                "このプラットフォームでは未対応です: file-preview",
            ),
            (
                FilePreviewError::Internal("file-preview"),
                "内部エラーが発生しました: file-preview",
            ),
            (
                FilePreviewError::Cancelled("file-preview"),
                "操作がキャンセルされました: file-preview",
            ),
            (
                FilePreviewError::PermissionDenied("file-preview"),
                "権限が拒否されました: file-preview",
            ),
            (
                FilePreviewError::InvalidInput("file-preview"),
                "入力が不正です: file-preview",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
