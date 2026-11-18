//! file-picker の失敗の種類。画面の状態型にも `fandhe-device` の `DeviceError` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

use fandhe_device::file_picker::PickError;

/// file-picker の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FilePickerError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: file_picker")]
    Unsupported,
    /// `DeviceError::InvalidInput` に相当する（サイズ超過など）。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
}

/// `fandhe-device::file_picker` の共有部品（読み取り・受け取りの検証）が返す `PickError` を、識別子ごと 1:1 に写す。
impl From<PickError> for FilePickerError {
    fn from(e: PickError) -> Self {
        match e {
            PickError::Unsupported(_) => Self::Unsupported,
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
                FilePickerError::Unsupported,
                "このプラットフォームでは未対応です: file_picker",
            ),
            (
                FilePickerError::InvalidInput("file_picker size"),
                "入力が不正です: file_picker size",
            ),
            (
                FilePickerError::Internal("file_picker read"),
                "内部エラーが発生しました: file_picker read",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }

    #[test]
    fn device_error_maps_one_to_one_with_identifiers() {
        assert_eq!(
            FilePickerError::from(PickError::InvalidInput("file_picker size")),
            FilePickerError::InvalidInput("file_picker size")
        );
        assert_eq!(
            FilePickerError::from(PickError::Internal("file_picker read")),
            FilePickerError::Internal("file_picker read")
        );
        assert_eq!(
            FilePickerError::from(PickError::Unsupported("file_picker")),
            FilePickerError::Unsupported
        );
    }
}
