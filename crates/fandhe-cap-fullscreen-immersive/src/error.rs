//! fullscreen-immersive の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// fullscreen-immersive の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FullscreenImmersiveError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: fullscreen-immersive")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: fullscreen-immersive")]
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                FullscreenImmersiveError::Unsupported,
                "このプラットフォームでは未対応です: fullscreen-immersive",
            ),
            (
                FullscreenImmersiveError::Internal,
                "内部エラーが発生しました: fullscreen-immersive",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
