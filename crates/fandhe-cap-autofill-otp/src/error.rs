//! autofill-otp の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// autofill-otp の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AutofillOtpError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: autofill-otp")]
    Unsupported,
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: autofill-otp")]
    PermissionDenied,
    /// `DeviceError::Cancelled` に相当する。
    #[error("操作がキャンセルされました: autofill-otp")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                AutofillOtpError::Unsupported,
                "このプラットフォームでは未対応です: autofill-otp",
            ),
            (
                AutofillOtpError::InvalidInput("autofill-otp"),
                "入力が不正です: autofill-otp",
            ),
            (
                AutofillOtpError::Internal("autofill-otp"),
                "内部エラーが発生しました: autofill-otp",
            ),
            (
                AutofillOtpError::PermissionDenied,
                "権限が拒否されました: autofill-otp",
            ),
            (
                AutofillOtpError::Cancelled,
                "操作がキャンセルされました: autofill-otp",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
