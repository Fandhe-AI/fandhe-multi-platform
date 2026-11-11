//! device-identifier の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// device-identifier の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeviceIdentifierError {
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: device-identifier")]
    Internal,
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: device-identifier")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: device-identifier")]
    PermissionDenied,
    /// `DeviceError::Cancelled` に相当する。
    #[error("操作がキャンセルされました: device-identifier")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                DeviceIdentifierError::Internal,
                "内部エラーが発生しました: device-identifier",
            ),
            (
                DeviceIdentifierError::Unsupported,
                "このプラットフォームでは未対応です: device-identifier",
            ),
            (
                DeviceIdentifierError::PermissionDenied,
                "権限が拒否されました: device-identifier",
            ),
            (
                DeviceIdentifierError::Cancelled,
                "操作がキャンセルされました: device-identifier",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
