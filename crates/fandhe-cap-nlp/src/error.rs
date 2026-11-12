//! nlp の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// nlp の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NlpError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: nlp")]
    Unsupported,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: nlp")]
    InvalidInput,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: nlp")]
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                NlpError::Unsupported,
                "このプラットフォームでは未対応です: nlp",
            ),
            (NlpError::InvalidInput, "入力が不正です: nlp"),
            (NlpError::Internal, "内部エラーが発生しました: nlp"),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
