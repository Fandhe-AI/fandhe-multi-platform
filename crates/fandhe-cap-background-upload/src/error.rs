//! background-upload の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// background-upload の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackgroundUploadError {
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Http` に相当する。送信先の URL の検査（https・ローカルだけ平文 http）に通らなかった失敗。
    /// 文言は検査の固定文で、URL を含めない。
    #[error("HTTP エラー: {0}")]
    Http(String),
}

/// 送信先 URL の検査（`fandhe_device::http::check_url`）の失敗を、この機能の失敗へ写す。
impl From<fandhe_device::DeviceError> for BackgroundUploadError {
    fn from(e: fandhe_device::DeviceError) -> Self {
        match e {
            fandhe_device::DeviceError::Http(m) => Self::Http(m),
            _ => Self::Internal("background upload"),
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
                BackgroundUploadError::Internal("background-upload"),
                "内部エラーが発生しました: background-upload",
            ),
            (
                BackgroundUploadError::Unsupported("background-upload"),
                "このプラットフォームでは未対応です: background-upload",
            ),
            (
                BackgroundUploadError::InvalidInput("background-upload"),
                "入力が不正です: background-upload",
            ),
            (BackgroundUploadError::Http("x".into()), "HTTP エラー: x"),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
