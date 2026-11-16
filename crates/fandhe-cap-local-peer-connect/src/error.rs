//! local-peer-connect の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// local-peer-connect の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LocalPeerConnectError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Disabled` に相当する。端末側（Wi-Fi のオフなど）で機能が無効なとき。呼び出し側が固定の識別子を渡す。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Cancelled` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                LocalPeerConnectError::Unsupported("local-peer-connect"),
                "このプラットフォームでは未対応です: local-peer-connect",
            ),
            (
                LocalPeerConnectError::PermissionDenied("local-peer-connect"),
                "権限が拒否されました: local-peer-connect",
            ),
            (
                LocalPeerConnectError::Disabled("local-peer-connect"),
                "端末側で機能が無効化されています: local-peer-connect",
            ),
            (
                LocalPeerConnectError::Internal("local-peer-connect"),
                "内部エラーが発生しました: local-peer-connect",
            ),
            (
                LocalPeerConnectError::Cancelled("local-peer-connect"),
                "操作がキャンセルされました: local-peer-connect",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
