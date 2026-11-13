//! managed-app-config の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// managed-app-config の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ManagedAppConfigError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Cancelled` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                ManagedAppConfigError::Unsupported("managed-app-config"),
                "このプラットフォームでは未対応です: managed-app-config",
            ),
            (
                ManagedAppConfigError::Internal("managed-app-config"),
                "内部エラーが発生しました: managed-app-config",
            ),
            (
                ManagedAppConfigError::PermissionDenied("managed-app-config"),
                "権限が拒否されました: managed-app-config",
            ),
            (
                ManagedAppConfigError::Cancelled("managed-app-config"),
                "操作がキャンセルされました: managed-app-config",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
