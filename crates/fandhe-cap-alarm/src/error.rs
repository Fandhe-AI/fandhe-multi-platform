//! alarm の失敗の種類。画面の状態型にも `fandhe-device` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の `DeviceError` と同じ書き出しで、OS の文言は持たない（ログ・`Debug` に出さない契約）。

/// alarm の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AlarmError {
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: alarm")]
    Unsupported,
    /// `DeviceError::PermissionDenied` に相当する。
    #[error("権限が拒否されました: alarm")]
    PermissionDenied,
    /// `DeviceError::InvalidInput` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Disabled` に相当する。OS 全体・グループポリシーで通知が無効なとき（Windows のトースト設定）。
    /// 呼び出し側が固定の識別子を渡す。
    #[error("端末側で機能が無効化されています: {0}")]
    Disabled(&'static str),
}

/// Windows の AUMID 登録・通知設定の写像（`fandhe-cap-notification`）の失敗を、種別ごと 1:1 に alarm の失敗へ写す。
/// 表示文言・識別子は移行前と変えない（呼び出し側が alarm のラベルを渡す）。
#[cfg(target_os = "windows")]
impl From<fandhe_cap_notification::NotificationError> for AlarmError {
    fn from(e: fandhe_cap_notification::NotificationError) -> Self {
        use fandhe_cap_notification::NotificationError as N;
        match e {
            N::InvalidInput(x) => AlarmError::InvalidInput(x),
            N::Unsupported(_) => AlarmError::Unsupported,
            N::Internal(x) => AlarmError::Internal(x),
            N::PermissionDenied(_) => AlarmError::PermissionDenied,
            N::Disabled(x) => AlarmError::Disabled(x),
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
                AlarmError::Unsupported,
                "このプラットフォームでは未対応です: alarm",
            ),
            (AlarmError::PermissionDenied, "権限が拒否されました: alarm"),
            (AlarmError::InvalidInput("alarm"), "入力が不正です: alarm"),
            (
                AlarmError::Internal("alarm"),
                "内部エラーが発生しました: alarm",
            ),
            (
                AlarmError::Disabled("alarm"),
                "端末側で機能が無効化されています: alarm",
            ),
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
