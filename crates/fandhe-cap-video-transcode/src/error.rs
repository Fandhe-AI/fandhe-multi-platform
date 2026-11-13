//! video-transcode の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// video-transcode の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VideoTranscodeError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: video-transcode")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                VideoTranscodeError::Unsupported,
                "このプラットフォームでは未対応です: video-transcode",
            ),
            (
                VideoTranscodeError::Internal("video-transcode"),
                "内部エラーが発生しました: video-transcode",
            ),
            (
                VideoTranscodeError::InvalidInput("video-transcode"),
                "入力が不正です: video-transcode",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
