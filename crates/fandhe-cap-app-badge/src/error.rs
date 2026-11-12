//! app-badge の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// app-badge の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AppBadgeError {
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: app-badge")]
    InvalidInput,
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: app-badge")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: app-badge")]
    PermissionDenied,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: app-badge")]
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (AppBadgeError::InvalidInput, "入力が不正です: app-badge"),
            (
                AppBadgeError::Unsupported,
                "このプラットフォームでは未対応です: app-badge",
            ),
            (
                AppBadgeError::PermissionDenied,
                "権限が拒否されました: app-badge",
            ),
            (
                AppBadgeError::Internal,
                "内部エラーが発生しました: app-badge",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
