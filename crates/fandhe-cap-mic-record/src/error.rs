//! mic-record の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と完全に一致させ（識別子は `mic_record`）、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// mic-record の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MicRecordError {
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: mic_record")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Cancelled` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: mic_record")]
    PermissionDenied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                MicRecordError::InvalidInput("mic_record"),
                "入力が不正です: mic_record",
            ),
            (
                MicRecordError::Unsupported,
                "このプラットフォームでは未対応です: mic_record",
            ),
            (
                MicRecordError::Internal("mic_record"),
                "内部エラーが発生しました: mic_record",
            ),
            (
                MicRecordError::Cancelled("mic_record"),
                "操作がキャンセルされました: mic_record",
            ),
            (
                MicRecordError::PermissionDenied,
                "権限が拒否されました: mic_record",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
