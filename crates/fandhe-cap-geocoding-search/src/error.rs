//! geocoding-search の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// geocoding-search の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GeocodingSearchError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: geocoding-search")]
    Unsupported,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: geocoding-search")]
    InvalidInput,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: geocoding-search")]
    PermissionDenied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                GeocodingSearchError::Unsupported,
                "このプラットフォームでは未対応です: geocoding-search",
            ),
            (
                GeocodingSearchError::InvalidInput,
                "入力が不正です: geocoding-search",
            ),
            (
                GeocodingSearchError::Internal("geocoding-search"),
                "内部エラーが発生しました: geocoding-search",
            ),
            (
                GeocodingSearchError::PermissionDenied,
                "権限が拒否されました: geocoding-search",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
