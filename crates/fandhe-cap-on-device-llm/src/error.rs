//! on-device-llm の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! `Internal` の識別子（`MODEL_NOT_READY` など）は契約（`contract`）が定める失敗の細分類で、画面側の
//! 分岐に使う。OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// on-device-llm の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OnDeviceLlmError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: on-device-llm")]
    Unsupported,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: on-device-llm")]
    InvalidInput,
    /// `DeviceError::Disabled` に相当する（OS 設定で Apple Intelligence が無効）。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
    /// `DeviceError::Internal` に相当する。識別子は契約の細分類（`MODEL_NOT_READY` ほか）。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: on-device-llm")]
    PermissionDenied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                OnDeviceLlmError::Unsupported,
                "このプラットフォームでは未対応です: on-device-llm",
            ),
            (
                OnDeviceLlmError::InvalidInput,
                "入力が不正です: on-device-llm",
            ),
            (
                OnDeviceLlmError::Disabled("on_device_llm"),
                "端末側で機能が無効化されています: on_device_llm",
            ),
            (
                OnDeviceLlmError::Internal("on_device_llm"),
                "内部エラーが発生しました: on_device_llm",
            ),
            (
                OnDeviceLlmError::PermissionDenied,
                "権限が拒否されました: on-device-llm",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
