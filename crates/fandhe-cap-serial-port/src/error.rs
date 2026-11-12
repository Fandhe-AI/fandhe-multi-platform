//! serial-port の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// serial-port の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SerialPortError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: serial-port")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: serial-port")]
    PermissionDenied,
    /// `DeviceError::Cancelled` に相当する。
    #[error("操作がキャンセルされました: serial-port")]
    Cancelled,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: serial-port")]
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                SerialPortError::Unsupported,
                "このプラットフォームでは未対応です: serial-port",
            ),
            (
                SerialPortError::PermissionDenied,
                "権限が拒否されました: serial-port",
            ),
            (
                SerialPortError::Cancelled,
                "操作がキャンセルされました: serial-port",
            ),
            (
                SerialPortError::Internal,
                "内部エラーが発生しました: serial-port",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
