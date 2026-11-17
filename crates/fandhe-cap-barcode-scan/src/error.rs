//! barcode-scan の失敗の種類。画面の状態型にも `fandhe-device` の `DeviceError` にも依存しないよう、機能 crate ごとに持つ（Issue #1355）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

use fandhe_device::DeviceError;

/// barcode-scan の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BarcodeScanError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::InvalidInput` に相当する（辺・画素数の上限超過）。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Cancelled` に相当する（写真選択のキャンセル。失敗として見せない）。呼び出し側が固定の識別子を渡す。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
}

/// `fandhe-device` の共有部品（`validate_photo`・`precheck_dimensions`）が返す `DeviceError` を、識別子ごと 1:1 に写す。
/// 共有部品が返さないバリアントは内部エラーへ畳む（OS の文言は持ち込まない）。
impl From<DeviceError> for BarcodeScanError {
    fn from(e: DeviceError) -> Self {
        match e {
            DeviceError::Unsupported(what) => Self::Unsupported(what),
            DeviceError::InvalidInput(what) => Self::InvalidInput(what),
            DeviceError::Internal(what) => Self::Internal(what),
            DeviceError::Cancelled(what) => Self::Cancelled(what),
            _ => Self::Internal("barcode_scan"),
        }
    }
}

/// 写真選択（`fandhe-cap-photo-picker`）の失敗を、識別子ごと 1:1 に写す。Android・wasm32 は写真選択の crate に依存しない。
#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
impl From<fandhe_cap_photo_picker::PhotoPickerError> for BarcodeScanError {
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
                BarcodeScanError::Unsupported("barcode_scan"),
                "このプラットフォームでは未対応です: barcode_scan",
            ),
            (
                BarcodeScanError::InvalidInput("photo_picker size"),
                "入力が不正です: photo_picker size",
            ),
            (
                BarcodeScanError::Cancelled("photo_picker"),
                "操作がキャンセルされました: photo_picker",
            ),
            (
                BarcodeScanError::Internal("barcode_scan"),
                "内部エラーが発生しました: barcode_scan",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }

    #[test]
    fn device_error_maps_one_to_one_with_identifiers() {
        assert_eq!(
            BarcodeScanError::from(DeviceError::InvalidInput("photo_picker size")),
            BarcodeScanError::InvalidInput("photo_picker size")
        );
        assert_eq!(
            BarcodeScanError::from(DeviceError::Internal("photo_picker header")),
            BarcodeScanError::Internal("photo_picker header")
        );
        assert_eq!(
            BarcodeScanError::from(DeviceError::Disabled("x")),
            BarcodeScanError::Internal("barcode_scan")
        );
    }
}
