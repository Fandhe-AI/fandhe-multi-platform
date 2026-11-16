//! file-association の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// file-association の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FileAssociationError {
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: file-association")]
    InvalidInput,
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: file-association")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: file-association")]
    PermissionDenied,
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                FileAssociationError::InvalidInput,
                "入力が不正です: file-association",
            ),
            (
                FileAssociationError::Unsupported,
                "このプラットフォームでは未対応です: file-association",
            ),
            (
                FileAssociationError::PermissionDenied,
                "権限が拒否されました: file-association",
            ),
            (
                FileAssociationError::Internal("file-association"),
                "内部エラーが発生しました: file-association",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
