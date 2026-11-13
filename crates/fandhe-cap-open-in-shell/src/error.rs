//! open-in-shell の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// open-in-shell の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OpenInShellError {
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
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
                OpenInShellError::InvalidInput("open-in-shell"),
                "入力が不正です: open-in-shell",
            ),
            (
                OpenInShellError::Unsupported("open-in-shell"),
                "このプラットフォームでは未対応です: open-in-shell",
            ),
            (
                OpenInShellError::Internal("open-in-shell"),
                "内部エラーが発生しました: open-in-shell",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
