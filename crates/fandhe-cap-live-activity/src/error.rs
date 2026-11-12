//! live-activity の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// live-activity の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveActivityError {
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: live_activity value")]
    InvalidInput,
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: live_activity")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: live_activity")]
    PermissionDenied,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Cancelled` に相当する。
    #[error("操作がキャンセルされました: live_activity")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                LiveActivityError::InvalidInput,
                "入力が不正です: live_activity value",
            ),
            (
                LiveActivityError::Unsupported,
                "このプラットフォームでは未対応です: live_activity",
            ),
            (
                LiveActivityError::PermissionDenied,
                "権限が拒否されました: live_activity",
            ),
            (
                LiveActivityError::Internal("live_activity start"),
                "内部エラーが発生しました: live_activity start",
            ),
            (
                LiveActivityError::Cancelled,
                "操作がキャンセルされました: live_activity",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
