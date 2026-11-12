//! midi の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// midi の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MidiError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: midi")]
    Unsupported,
    /// `DeviceError::Internal` に相当する。
    #[error("内部エラーが発生しました: midi")]
    Internal,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: midi")]
    PermissionDenied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
            (
                MidiError::Unsupported,
                "このプラットフォームでは未対応です: midi",
            ),
            (MidiError::Internal, "内部エラーが発生しました: midi"),
            (MidiError::PermissionDenied, "権限が拒否されました: midi"),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
