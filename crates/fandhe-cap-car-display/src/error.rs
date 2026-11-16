//! car-display の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// car-display の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CarDisplayError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: car-display")]
    Unsupported,
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: car-display")]
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                CarDisplayError::Unsupported,
                "このプラットフォームでは未対応です: car-display",
            ),
            (
                CarDisplayError::InvalidInput("car-display"),
                "入力が不正です: car-display",
            ),
            (
                CarDisplayError::Internal,
                "内部エラーが発生しました: car-display",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
