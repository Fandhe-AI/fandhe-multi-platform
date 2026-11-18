//! image-recognition の失敗の種類。画面の状態型にも `fandhe-device` の `DeviceError` にも依存しないよう、機能 crate ごとに持つ（Issue #1355）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

use fandhe_device::file_picker::PickError;

/// image-recognition の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ImageRecognitionError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す（認識言語が使えないときは `LANGUAGE_UNAVAILABLE`）。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::InvalidInput` に相当する（辺・画素数の上限超過）。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Cancelled` に相当する（写真選択のキャンセル。失敗として見せない）。呼び出し側が固定の識別子を渡す。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
}

/// `fandhe-device` の共有部品（`validate_photo`・`precheck_dimensions`）が返す `PickError` を、識別子ごと 1:1 に写す。
/// 共有部品が返さないバリアントは内部エラーへ畳む（OS の文言は持ち込まない）。
impl From<PickError> for ImageRecognitionError {
    fn from(e: PickError) -> Self {
        match e {
            PickError::Unsupported(what) => Self::Unsupported(what),
            PickError::InvalidInput(what) => Self::InvalidInput(what),
            PickError::Internal(what) => Self::Internal(what),
        }
    }
}

/// 写真選択（`fandhe-cap-photo-picker`）の失敗を、識別子ごと 1:1 に写す。Android・wasm32 は写真選択の crate に依存しない。
#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
impl From<fandhe_cap_photo_picker::PhotoPickerError> for ImageRecognitionError {
    fn from(e: fandhe_cap_photo_picker::PhotoPickerError) -> Self {
        use fandhe_cap_photo_picker::PhotoPickerError as P;
        match e {
            P::Unsupported(what) => Self::Unsupported(what),
            P::InvalidInput(what) => Self::InvalidInput(what),
            P::Internal(what) => Self::Internal(what),
            P::Cancelled(what) => Self::Cancelled(what),
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
                ImageRecognitionError::Unsupported("image_recognition"),
                "このプラットフォームでは未対応です: image_recognition",
            ),
            (
                ImageRecognitionError::InvalidInput("image_recognition size"),
                "入力が不正です: image_recognition size",
            ),
            (
                ImageRecognitionError::Internal("image_recognition header"),
                "内部エラーが発生しました: image_recognition header",
            ),
            (
                ImageRecognitionError::Cancelled("photo_picker"),
                "操作がキャンセルされました: photo_picker",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }

    #[test]
    fn device_error_maps_one_to_one_with_identifiers() {
        assert_eq!(
            ImageRecognitionError::from(PickError::InvalidInput("photo_picker size")),
            ImageRecognitionError::InvalidInput("photo_picker size")
        );
        assert_eq!(
            ImageRecognitionError::from(PickError::Internal("photo_picker header")),
            ImageRecognitionError::Internal("photo_picker header")
        );
    }
}
