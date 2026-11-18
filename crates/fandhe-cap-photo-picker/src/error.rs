//! photo-picker の失敗の種類。画面の状態型にも `fandhe-device` の `DeviceError` にも依存しないよう、機能 crate ごとに持つ（Issue #1355）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、識別子は `photo_picker` のまま固定する（画面に出る文言を変えない）。
//! OS の文言は持たない（ログ・`Debug` に出さない契約）。

use fandhe_device::file_picker::PickError;

/// photo-picker の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PhotoPickerError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::InvalidInput` に相当する（サイズ超過など）。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Cancelled` に相当する（利用者のキャンセル。失敗として見せない）。呼び出し側が固定の識別子を渡す。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
}

/// `fandhe-device` の共有部品（`validate_photo`・読み取り・受け取りの検証）が返す `PickError` を、識別子ごと 1:1 に写す。
impl From<PickError> for PhotoPickerError {
    fn from(e: PickError) -> Self {
        match e {
            PickError::Unsupported(what) => Self::Unsupported(what),
            PickError::InvalidInput(what) => Self::InvalidInput(what),
            PickError::Internal(what) => Self::Internal(what),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                PhotoPickerError::Unsupported("photo_picker"),
                "このプラットフォームでは未対応です: photo_picker",
            ),
            (
                PhotoPickerError::InvalidInput("photo_picker size"),
                "入力が不正です: photo_picker size",
            ),
            (
                PhotoPickerError::Internal("photo_picker format"),
                "内部エラーが発生しました: photo_picker format",
            ),
            (
                PhotoPickerError::Cancelled("photo_picker"),
                "操作がキャンセルされました: photo_picker",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }

    #[test]
    fn device_error_maps_one_to_one_with_identifiers() {
        assert_eq!(
            PhotoPickerError::from(PickError::InvalidInput("photo_picker size")),
            PhotoPickerError::InvalidInput("photo_picker size")
        );
        assert_eq!(
            PhotoPickerError::from(PickError::Internal("photo_picker empty")),
            PhotoPickerError::Internal("photo_picker empty")
        );
        assert_eq!(
            PhotoPickerError::from(PickError::Unsupported("photo_picker")),
            PhotoPickerError::Unsupported("photo_picker")
        );
    }
}
