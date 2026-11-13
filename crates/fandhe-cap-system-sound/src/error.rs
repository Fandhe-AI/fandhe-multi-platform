//! system-sound の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// system-sound の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SystemSoundError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: system-sound")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: system-sound")]
    PermissionDenied,
    /// `DeviceError::Cancelled` に相当する。
    #[error("操作がキャンセルされました: system-sound")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                SystemSoundError::Unsupported,
                "このプラットフォームでは未対応です: system-sound",
            ),
            (
                SystemSoundError::Internal("system-sound"),
                "内部エラーが発生しました: system-sound",
            ),
            (
                SystemSoundError::PermissionDenied,
                "権限が拒否されました: system-sound",
            ),
            (
                SystemSoundError::Cancelled,
                "操作がキャンセルされました: system-sound",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
