//! in-app-review の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// in-app-review の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InAppReviewError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: in-app-review")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: in-app-review")]
    Internal,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: in-app-review")]
    PermissionDenied,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: in-app-review")]
    InvalidInput,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                InAppReviewError::Unsupported,
                "このプラットフォームでは未対応です: in-app-review",
            ),
            (
                InAppReviewError::Internal,
                "内部エラーが発生しました: in-app-review",
            ),
            (
                InAppReviewError::PermissionDenied,
                "権限が拒否されました: in-app-review",
            ),
            (
                InAppReviewError::InvalidInput,
                "入力が不正です: in-app-review",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
