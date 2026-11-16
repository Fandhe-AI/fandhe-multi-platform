//! url-scheme の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// url-scheme の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UrlSchemeError {
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: url-scheme")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                UrlSchemeError::InvalidInput("url-scheme"),
                "入力が不正です: url-scheme",
            ),
            (
                UrlSchemeError::Unsupported,
                "このプラットフォームでは未対応です: url-scheme",
            ),
            (
                UrlSchemeError::PermissionDenied("url-scheme"),
                "権限が拒否されました: url-scheme",
            ),
            (
                UrlSchemeError::Internal("url-scheme"),
                "内部エラーが発生しました: url-scheme",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
