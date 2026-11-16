//! payment-request の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// payment-request の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PaymentRequestError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: payment-request")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: payment-request")]
    PermissionDenied,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: payment-request")]
    InvalidInput,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                PaymentRequestError::Unsupported,
                "このプラットフォームでは未対応です: payment-request",
            ),
            (
                PaymentRequestError::Internal("payment-request"),
                "内部エラーが発生しました: payment-request",
            ),
            (
                PaymentRequestError::PermissionDenied,
                "権限が拒否されました: payment-request",
            ),
            (
                PaymentRequestError::InvalidInput,
                "入力が不正です: payment-request",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
