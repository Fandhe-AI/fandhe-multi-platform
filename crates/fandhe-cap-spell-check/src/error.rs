//! spell-check の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。
//! `Internal` / `Disabled` は識別子（`TIMEOUT`・`LANGUAGE_UNAVAILABLE` など）で画面の失敗種別へ分岐するため、
//! 元の `DeviceError` と同じく `&'static str` を持つ。

/// spell-check の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpellCheckError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: spell-check")]
    Unsupported,
    /// `DeviceError::InvalidInput` に相当する。
    #[error("入力が不正です: spell-check")]
    InvalidInput,
    /// `DeviceError::Internal` に相当する。識別子は契約の定数（`TIMEOUT` など）。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Disabled` に相当する（OS 側でスペルチェッカーが無効）。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: spell-check")]
    PermissionDenied,
    /// `DeviceError::Cancelled` に相当する。
    #[error("操作がキャンセルされました: spell-check")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                SpellCheckError::Unsupported,
                "このプラットフォームでは未対応です: spell-check",
            ),
            (SpellCheckError::InvalidInput, "入力が不正です: spell-check"),
            (
                SpellCheckError::Internal("spell_check"),
                "内部エラーが発生しました: spell_check",
            ),
            (
                SpellCheckError::Disabled("spell_check"),
                "端末側で機能が無効化されています: spell_check",
            ),
            (
                SpellCheckError::PermissionDenied,
                "権限が拒否されました: spell-check",
            ),
            (
                SpellCheckError::Cancelled,
                "操作がキャンセルされました: spell-check",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
