//! notification の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// notification の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NotificationError {
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Disabled` に相当する。OS 全体・グループポリシーで通知が無効なとき。呼び出し側が固定の識別子を渡す。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                NotificationError::InvalidInput("notification"),
                "入力が不正です: notification",
            ),
            (
                NotificationError::Unsupported("notification"),
                "このプラットフォームでは未対応です: notification",
            ),
            (
                NotificationError::Internal("notification"),
                "内部エラーが発生しました: notification",
            ),
            (
                NotificationError::PermissionDenied("notification"),
                "権限が拒否されました: notification",
            ),
            (
                NotificationError::Disabled("notification"),
                "端末側で機能が無効化されています: notification",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
