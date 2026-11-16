//! now-playing-controls の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// now-playing-controls の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NowPlayingControlsError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: now-playing-controls")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: now-playing-controls")]
    Internal,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: now-playing-controls")]
    InvalidInput,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                NowPlayingControlsError::Unsupported,
                "このプラットフォームでは未対応です: now-playing-controls",
            ),
            (
                NowPlayingControlsError::Internal,
                "内部エラーが発生しました: now-playing-controls",
            ),
            (
                NowPlayingControlsError::InvalidInput,
                "入力が不正です: now-playing-controls",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
