//! speech-recognition の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と完全に一致させ（識別子は `speech_recognition`）、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// speech-recognition の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpeechRecognitionError {
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: speech_recognition")]
    PermissionDenied,
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: speech_recognition")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Cancelled` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
    /// `DeviceError::Disabled` に相当する（OS の設定で音声認識が無効化されている）。呼び出し側が固定の識別子を渡す。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                SpeechRecognitionError::PermissionDenied,
                "権限が拒否されました: speech_recognition",
            ),
            (
                SpeechRecognitionError::Unsupported,
                "このプラットフォームでは未対応です: speech_recognition",
            ),
            (
                SpeechRecognitionError::Internal("speech_recognition"),
                "内部エラーが発生しました: speech_recognition",
            ),
            (
                SpeechRecognitionError::InvalidInput("speech_recognition"),
                "入力が不正です: speech_recognition",
            ),
            (
                SpeechRecognitionError::Cancelled("speech_recognition"),
                "操作がキャンセルされました: speech_recognition",
            ),
            (
                SpeechRecognitionError::Disabled("speech_recognition"),
                "端末側で機能が無効化されています: speech_recognition",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
