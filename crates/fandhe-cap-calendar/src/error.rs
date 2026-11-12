//! calendar の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// calendar の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CalendarError {
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: calendar")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: calendar")]
    PermissionDenied,
    /// `DeviceError::Internal` に相当する。
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
                CalendarError::InvalidInput("calendar title"),
                "入力が不正です: calendar title",
            ),
            (
                CalendarError::Unsupported,
                "このプラットフォームでは未対応です: calendar",
            ),
            (
                CalendarError::PermissionDenied,
                "権限が拒否されました: calendar",
            ),
            (
                CalendarError::Internal("calendar save"),
                "内部エラーが発生しました: calendar save",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
