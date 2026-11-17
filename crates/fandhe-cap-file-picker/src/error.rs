//! file-picker の失敗の種類。画面の状態型にも `fandhe-device` の `DeviceError` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

use fandhe_device::DeviceError;

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

/// `fandhe-device::file_picker` の共有部品（読み取り・受け取りの検証）が返す `DeviceError` を、識別子ごと 1:1 に写す。
/// 共有部品が返さないバリアント（`Io` など）は内部エラーへ畳む（OS の文言は持ち込まない）。
impl From<DeviceError> for FilePickerError {
    fn from(e: DeviceError) -> Self {
        match e {
            DeviceError::Unsupported(_) => Self::Unsupported,
            DeviceError::InvalidInput(what) => Self::InvalidInput(what),
            DeviceError::Internal(what) => Self::Internal(what),
            _ => Self::Internal("file_picker"),
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
            FilePickerError::from(DeviceError::InvalidInput("file_picker size")),
            FilePickerError::InvalidInput("file_picker size")
        );
        assert_eq!(
            FilePickerError::from(DeviceError::Internal("file_picker read")),
            FilePickerError::Internal("file_picker read")
        );
        assert_eq!(
            FilePickerError::from(DeviceError::Unsupported("file_picker")),
            FilePickerError::Unsupported
        );
        // 共有部品が返さないバリアントは内部エラーへ畳み、成功にも別種にもしない。
        assert_eq!(
            FilePickerError::from(DeviceError::Disabled("x")),
            FilePickerError::Internal("file_picker")
        );
    }
}
