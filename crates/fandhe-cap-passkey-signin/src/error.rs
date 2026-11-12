//! passkey-signin の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// passkey-signin の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PasskeySigninError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: passkey-signin")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: passkey-signin")]
    Internal,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: passkey-signin")]
    InvalidInput,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                PasskeySigninError::Unsupported,
                "このプラットフォームでは未対応です: passkey-signin",
            ),
            (
                PasskeySigninError::Internal,
                "内部エラーが発生しました: passkey-signin",
            ),
            (
                PasskeySigninError::InvalidInput,
                "入力が不正です: passkey-signin",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
