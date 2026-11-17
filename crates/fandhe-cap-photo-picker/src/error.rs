//! photo-picker の失敗の種類。画面の状態型にも `fandhe-device` の `DeviceError` にも依存しないよう、機能 crate ごとに持つ（Issue #1355）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、識別子は `photo_picker` のまま固定する（画面に出る文言を変えない）。
//! OS の文言は持たない（ログ・`Debug` に出さない契約）。

use fandhe_device::DeviceError;

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

/// `fandhe-device` の共有部品（`validate_photo`・読み取り・受け取りの検証）が返す `DeviceError` を、識別子ごと 1:1 に写す。
/// 共有部品が返さないバリアント（`Io` など）は内部エラーへ畳む（OS の文言は持ち込まない）。
impl From<DeviceError> for PhotoPickerError {
    fn from(e: DeviceError) -> Self {
        match e {
            DeviceError::Unsupported(what) => Self::Unsupported(what),
            DeviceError::InvalidInput(what) => Self::InvalidInput(what),
            DeviceError::Internal(what) => Self::Internal(what),
            DeviceError::Cancelled(what) => Self::Cancelled(what),
            _ => Self::Internal("photo_picker"),
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
            PhotoPickerError::from(DeviceError::InvalidInput("photo_picker size")),
            PhotoPickerError::InvalidInput("photo_picker size")
        );
        assert_eq!(
            PhotoPickerError::from(DeviceError::Internal("photo_picker empty")),
            PhotoPickerError::Internal("photo_picker empty")
        );
        assert_eq!(
            PhotoPickerError::from(DeviceError::Unsupported("photo_picker")),
            PhotoPickerError::Unsupported("photo_picker")
        );
        assert_eq!(
            PhotoPickerError::from(DeviceError::Cancelled("photo_picker")),
            PhotoPickerError::Cancelled("photo_picker")
        );
        // 共有部品が返さないバリアントは内部エラーへ畳み、成功にも別種にもしない。
        assert_eq!(
            PhotoPickerError::from(DeviceError::Disabled("x")),
            PhotoPickerError::Internal("photo_picker")
        );
    }
}
