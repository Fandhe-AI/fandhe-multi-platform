//! secure-credential の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// secure-credential の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecureCredentialError {
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::InvalidKey` に相当する。key は秘密値ではないため、表示は固定文言の後ろに key を付ける。
    #[error("不正な key: {0}")]
    InvalidKey(String),
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: secure-credential")]
    Unsupported,
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
                SecureCredentialError::InvalidInput("secure-credential"),
                "入力が不正です: secure-credential",
            ),
            (
                SecureCredentialError::InvalidKey("a/b".into()),
                "不正な key: a/b",
            ),
            (
                SecureCredentialError::Unsupported,
                "このプラットフォームでは未対応です: secure-credential",
            ),
            (
                SecureCredentialError::Internal("secure-credential"),
                "内部エラーが発生しました: secure-credential",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
