//! http-get の失敗の種類。`fandhe-device` の `DeviceError` に依存しないよう機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError::Http` と同じ。メッセージに URL 全体・応答本文は含めない（security.md A09）。

/// http-get の失敗。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpGetError {
    /// 通信・許可 URL 検証・応答上限超過などの失敗。
    #[error("HTTP エラー: {0}")]
    Http(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_is_fixed() {
        assert_eq!(HttpGetError::Http("x".into()).to_string(), "HTTP エラー: x");
    }
}
