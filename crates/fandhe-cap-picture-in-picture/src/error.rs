//! picture-in-picture の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// picture-in-picture の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PictureInPictureError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: picture-in-picture")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Disabled` に相当する。端末側（OS の設定・状態）で機能が無効なとき。呼び出し側が固定の識別子を渡す。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
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
                PictureInPictureError::Unsupported,
                "このプラットフォームでは未対応です: picture-in-picture",
            ),
            (
                PictureInPictureError::PermissionDenied("picture-in-picture"),
                "権限が拒否されました: picture-in-picture",
            ),
            (
                PictureInPictureError::Disabled("picture-in-picture"),
                "端末側で機能が無効化されています: picture-in-picture",
            ),
            (
                PictureInPictureError::Internal("picture-in-picture"),
                "内部エラーが発生しました: picture-in-picture",
            ),
            (
                PictureInPictureError::InvalidInput("picture-in-picture"),
                "入力が不正です: picture-in-picture",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
