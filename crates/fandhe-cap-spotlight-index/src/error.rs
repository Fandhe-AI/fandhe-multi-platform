//! spotlight-index の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// spotlight-index の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpotlightIndexError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: spotlight-index")]
    PermissionDenied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                SpotlightIndexError::Unsupported("spotlight-index"),
                "このプラットフォームでは未対応です: spotlight-index",
            ),
            (
                SpotlightIndexError::InvalidInput("spotlight-index"),
                "入力が不正です: spotlight-index",
            ),
            (
                SpotlightIndexError::Internal("spotlight-index"),
                "内部エラーが発生しました: spotlight-index",
            ),
            (
                SpotlightIndexError::PermissionDenied,
                "権限が拒否されました: spotlight-index",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
