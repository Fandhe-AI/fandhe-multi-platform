//! map-view の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// map-view の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MapViewError {
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: map-view")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Disabled` に相当する。端末側（通信の無効化など）で機能が使えないとき。呼び出し側が固定の識別子を渡す。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
    /// `DeviceError::InvalidImageSize` に相当する。取得画像の寸法が要求と合わないとき。
    #[error("画像サイズが不正または大きすぎます: {0}x{1}")]
    InvalidImageSize(u32, u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                MapViewError::InvalidInput("map-view"),
                "入力が不正です: map-view",
            ),
            (
                MapViewError::Unsupported,
                "このプラットフォームでは未対応です: map-view",
            ),
            (
                MapViewError::Internal("map-view"),
                "内部エラーが発生しました: map-view",
            ),
            (
                MapViewError::Disabled("map-view"),
                "端末側で機能が無効化されています: map-view",
            ),
            (
                MapViewError::InvalidImageSize(3, 2),
                "画像サイズが不正または大きすぎます: 3x2",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
