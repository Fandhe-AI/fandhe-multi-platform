//! capture-protection の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// capture-protection の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CaptureProtectionError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: capture-protection")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: capture-protection")]
    PermissionDenied,
    /// `DeviceError::Cancelled` に相当する。
    #[error("操作がキャンセルされました: capture-protection")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                CaptureProtectionError::Unsupported,
                "このプラットフォームでは未対応です: capture-protection",
            ),
            (
                CaptureProtectionError::Internal("capture-protection"),
                "内部エラーが発生しました: capture-protection",
            ),
            (
                CaptureProtectionError::PermissionDenied,
                "権限が拒否されました: capture-protection",
            ),
            (
                CaptureProtectionError::Cancelled,
                "操作がキャンセルされました: capture-protection",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
