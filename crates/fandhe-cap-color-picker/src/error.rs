//! color-picker の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// color-picker の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ColorPickerError {
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Cancelled` に相当する。
    #[error("操作がキャンセルされました: color-picker")]
    Cancelled,
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: color-picker")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: color-picker")]
    PermissionDenied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                ColorPickerError::Internal("color-picker"),
                "内部エラーが発生しました: color-picker",
            ),
            (
                ColorPickerError::Cancelled,
                "操作がキャンセルされました: color-picker",
            ),
            (
                ColorPickerError::Unsupported,
                "このプラットフォームでは未対応です: color-picker",
            ),
            (
                ColorPickerError::PermissionDenied,
                "権限が拒否されました: color-picker",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
