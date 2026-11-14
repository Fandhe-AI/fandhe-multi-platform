//! launch-at-login の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// launch-at-login の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LaunchAtLoginError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: launch-at-login")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
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
                LaunchAtLoginError::Unsupported,
                "このプラットフォームでは未対応です: launch-at-login",
            ),
            (
                LaunchAtLoginError::PermissionDenied("launch-at-login"),
                "権限が拒否されました: launch-at-login",
            ),
            (
                LaunchAtLoginError::Internal("launch-at-login"),
                "内部エラーが発生しました: launch-at-login",
            ),
            (
                LaunchAtLoginError::InvalidInput("launch-at-login"),
                "入力が不正です: launch-at-login",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
