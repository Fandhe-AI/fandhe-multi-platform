//! screen-capture の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// screen-capture の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScreenCaptureError {
    /// `DeviceError::Cancelled` に相当する。
    #[error("操作がキャンセルされました: screen-capture")]
    Cancelled,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: screen-capture")]
    InvalidInput,
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: screen-capture")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: screen-capture")]
    PermissionDenied,
    /// `DeviceError::Internal` に相当する。
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
                ScreenCaptureError::Cancelled,
                "操作がキャンセルされました: screen-capture",
            ),
            (
                ScreenCaptureError::InvalidInput,
                "入力が不正です: screen-capture",
            ),
            (
                ScreenCaptureError::Unsupported,
                "このプラットフォームでは未対応です: screen-capture",
            ),
            (
                ScreenCaptureError::PermissionDenied,
                "権限が拒否されました: screen-capture",
            ),
            (
                ScreenCaptureError::Internal("sample"),
                "内部エラーが発生しました: sample",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
