//! ml-inference の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// ml-inference の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MlInferenceError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: ml_inference")]
    Unsupported,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: ml_inference input")]
    InvalidInput,
    /// `DeviceError::Internal` に相当する。payload は内部識別子（`INVALID_MODEL` など）だけで、OS の文言は入れない。
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
                MlInferenceError::Unsupported,
                "このプラットフォームでは未対応です: ml_inference",
            ),
            (
                MlInferenceError::InvalidInput,
                "入力が不正です: ml_inference input",
            ),
            (
                MlInferenceError::Internal("ml_inference"),
                "内部エラーが発生しました: ml_inference",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
