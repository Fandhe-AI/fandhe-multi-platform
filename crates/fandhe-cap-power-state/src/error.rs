//! power-state の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// power-state の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PowerStateError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: power_state")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: power_state read")]
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                PowerStateError::Unsupported,
                "このプラットフォームでは未対応です: power_state",
            ),
            (
                PowerStateError::Internal,
                "内部エラーが発生しました: power_state read",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
