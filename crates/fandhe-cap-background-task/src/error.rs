//! background-task の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// background-task の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackgroundTaskError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Disabled` に相当する。端末側（OS の設定）でバックグラウンド実行が無効なとき。呼び出し側が固定の識別子を渡す。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
    /// 実行記録の保存層（`Storage`）の失敗。`StorageError` の文言（キー不正・入出力など）をそのまま転送する
    /// （OS の内部構造は出さず `Display` の文言だけ）。
    #[error("{0}")]
    Storage(String),
}

impl From<fandhe_device::storage::StorageError> for BackgroundTaskError {
    /// `Storage` の失敗を写す。未対応は同じ種別へ、それ以外（キー不正・入出力）は文言を転送する。
    fn from(e: fandhe_device::storage::StorageError) -> Self {
        use fandhe_device::storage::StorageError;
        match e {
            StorageError::Unsupported(s) => Self::Unsupported(s),
            other => Self::Storage(other.to_string()),
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
                BackgroundTaskError::Unsupported("background-task"),
                "このプラットフォームでは未対応です: background-task",
            ),
            (
                BackgroundTaskError::Internal("background-task"),
                "内部エラーが発生しました: background-task",
            ),
            (
                BackgroundTaskError::PermissionDenied("background-task"),
                "権限が拒否されました: background-task",
            ),
            (
                BackgroundTaskError::Disabled("background-task"),
                "端末側で機能が無効化されています: background-task",
            ),
            (
                BackgroundTaskError::Storage("入出力エラー: x".to_string()),
                "入出力エラー: x",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
