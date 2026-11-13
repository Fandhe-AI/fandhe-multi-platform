//! removable-storage の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// removable-storage の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RemovableStorageError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                RemovableStorageError::Unsupported("removable-storage"),
                "このプラットフォームでは未対応です: removable-storage",
            ),
            (
                RemovableStorageError::PermissionDenied("removable-storage"),
                "権限が拒否されました: removable-storage",
            ),
            (
                RemovableStorageError::Internal("removable-storage"),
                "内部エラーが発生しました: removable-storage",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
