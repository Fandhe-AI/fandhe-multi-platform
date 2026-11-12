//! app-attest の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// app-attest の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AppAttestError {
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: app-attest")]
    InvalidInput,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: app-attest")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: app-attest")]
    PermissionDenied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (AppAttestError::InvalidInput, "入力が不正です: app-attest"),
            (
                AppAttestError::Internal("app-attest"),
                "内部エラーが発生しました: app-attest",
            ),
            (
                AppAttestError::Unsupported,
                "このプラットフォームでは未対応です: app-attest",
            ),
            (
                AppAttestError::PermissionDenied,
                "権限が拒否されました: app-attest",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
