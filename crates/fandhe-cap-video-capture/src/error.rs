//! video-capture の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。
//!
//! 撮影の結果は契約の `VideoCaptureFailure`（理由別）で返すため、本型を返す口は現時点で無い。
//! 他の機能 crate と同じ形（`<Pascal>Error`）で公開入口を揃えるために持つ。

/// video-capture の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VideoCaptureError {
    /// この OS・環境では撮影できない。
    #[error("video capture is unsupported")]
    Unsupported,
    /// OS が失敗を返した。理由・OS の文言は持たない。
    #[error("video capture failed")]
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        assert_eq!(
            VideoCaptureError::Unsupported.to_string(),
            "video capture is unsupported"
        );
        assert_eq!(
            VideoCaptureError::Internal.to_string(),
            "video capture failed"
        );
    }
}
