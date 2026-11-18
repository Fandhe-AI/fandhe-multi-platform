//! photo-library の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// photo-library の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PhotoLibraryError {
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
}

impl From<fandhe_device::photo_library::InvalidPng> for PhotoLibraryError {
    fn from(_: fandhe_device::photo_library::InvalidPng) -> Self {
        Self::InvalidInput("photo_library png")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                PhotoLibraryError::PermissionDenied("photo-library"),
                "権限が拒否されました: photo-library",
            ),
            (
                PhotoLibraryError::Unsupported("photo-library"),
                "このプラットフォームでは未対応です: photo-library",
            ),
            (
                PhotoLibraryError::InvalidInput("photo-library"),
                "入力が不正です: photo-library",
            ),
            (
                PhotoLibraryError::Internal("photo-library"),
                "内部エラーが発生しました: photo-library",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
