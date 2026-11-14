//! wifi-info の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// wifi-info の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WifiInfoError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Disabled` に相当する（Wi-Fi の電源・位置情報サービスがオフ）。呼び出し側が固定の識別子を渡す。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                WifiInfoError::Unsupported("wifi-info"),
                "このプラットフォームでは未対応です: wifi-info",
            ),
            (
                WifiInfoError::PermissionDenied("wifi-info"),
                "権限が拒否されました: wifi-info",
            ),
            (
                WifiInfoError::Internal("wifi-info"),
                "内部エラーが発生しました: wifi-info",
            ),
            (
                WifiInfoError::Disabled("wifi-info"),
                "端末側で機能が無効化されています: wifi-info",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
