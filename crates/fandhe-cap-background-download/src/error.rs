//! background-download の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// background-download の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackgroundDownloadError {
    /// `DeviceError::Unsupported` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// `DeviceError::Http` に相当する。保存元 URL の検査（https・ローカルだけ平文 http）に通らなかった失敗。
    /// 文言は検査の固定文で、URL を含めない。
    #[error("HTTP エラー: {0}")]
    Http(String),
    /// `DeviceError::Io` に相当する。保存先の準備・削除の失敗（Windows の BITS）。`std::io::Error` の `Display` だけを持つ。
    #[error("入出力エラー: {0}")]
    Io(String),
}

impl From<std::io::Error> for BackgroundDownloadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// 保存元 URL の検査（`fandhe_device::http::check_url`）の失敗を、この機能の失敗へ写す。
impl From<fandhe_device::DeviceError> for BackgroundDownloadError {
    fn from(e: fandhe_device::DeviceError) -> Self {
        match e {
            fandhe_device::DeviceError::Http(m) => Self::Http(m),
            _ => Self::Internal("background download"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (BackgroundDownloadError::Io("x".into()), "入出力エラー: x"),
            (BackgroundDownloadError::Http("x".into()), "HTTP エラー: x"),
            (
                BackgroundDownloadError::Unsupported("background-download"),
                "このプラットフォームでは未対応です: background-download",
            ),
            (
                BackgroundDownloadError::Internal("background-download"),
                "内部エラーが発生しました: background-download",
            ),
            (
                BackgroundDownloadError::InvalidInput("background-download"),
                "入力が不正です: background-download",
            ),
            (
                BackgroundDownloadError::PermissionDenied("background-download"),
                "権限が拒否されました: background-download",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
