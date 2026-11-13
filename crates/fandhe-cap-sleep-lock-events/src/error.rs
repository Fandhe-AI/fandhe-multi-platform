//! sleep-lock-events の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// sleep-lock-events の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SleepLockEventsError {
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
                SleepLockEventsError::Unsupported("sleep-lock-events"),
                "このプラットフォームでは未対応です: sleep-lock-events",
            ),
            (
                SleepLockEventsError::Internal("sleep-lock-events"),
                "内部エラーが発生しました: sleep-lock-events",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
