//! recent-documents の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// recent-documents の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecentDocumentsError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
}

/// open-in-shell の検査（`ShellPath`・サンプルの作成・パスの正規化）の失敗を、識別子ごと 1:1 で写す
/// （画面の写像が `"shell sample"`・`"shell path"` を見るため、識別子を変えない）。
impl From<fandhe_cap_open_in_shell::OpenInShellError> for RecentDocumentsError {
    fn from(e: fandhe_cap_open_in_shell::OpenInShellError) -> Self {
        use fandhe_cap_open_in_shell::OpenInShellError as S;
        match e {
            S::InvalidInput(s) => Self::InvalidInput(s),
            S::Unsupported(s) => Self::Unsupported(s),
            S::Internal(s) => Self::Internal(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                RecentDocumentsError::Unsupported("recent-documents"),
                "このプラットフォームでは未対応です: recent-documents",
            ),
            (
                RecentDocumentsError::Internal("recent-documents"),
                "内部エラーが発生しました: recent-documents",
            ),
            (
                RecentDocumentsError::InvalidInput("recent-documents"),
                "入力が不正です: recent-documents",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }

    #[test]
    fn open_in_shell_errors_keep_their_identifiers() {
        use fandhe_cap_open_in_shell::OpenInShellError as S;
        assert_eq!(
            RecentDocumentsError::from(S::InvalidInput("shell path")),
            RecentDocumentsError::InvalidInput("shell path")
        );
        assert_eq!(
            RecentDocumentsError::from(S::Internal("shell sample")),
            RecentDocumentsError::Internal("shell sample")
        );
        assert_eq!(
            RecentDocumentsError::from(S::Unsupported("x")),
            RecentDocumentsError::Unsupported("x")
        );
    }
}
