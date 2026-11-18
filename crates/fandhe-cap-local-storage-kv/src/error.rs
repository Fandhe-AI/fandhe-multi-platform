//! local-storage-kv の失敗の種類。保存先を決められないときだけを表す（読み書きの失敗は `Storage` が
//! `fandhe_device::storage::StorageError` で返す）。表示文言は移行前の `DeviceError::Unsupported` と同じ。

/// local-storage-kv の失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LocalStorageKvError {
    /// 保存先（ホームディレクトリ・内部ストレージ）を取得できない。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        assert_eq!(
            LocalStorageKvError::Unsupported("home directory not found").to_string(),
            "このプラットフォームでは未対応です: home directory not found"
        );
    }
}
