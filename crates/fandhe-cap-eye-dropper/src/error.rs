//! eye-dropper の失敗の種類。`RequestSlot` は結果に `Clone` を要るが、契約側の `DeviceError` は `Clone` ではないため、
//! 機能 crate ごとに持つ（Issue #1183・#1359）。表示文言は `DeviceError` と同じ書き出しで、OS の文言・拾った色は持たない
//! （ログ・`Debug` に出さない契約）。

use fandhe_device::DeviceError;

/// eye-dropper の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EyeDropperError {
    /// `DeviceError::Internal` に相当する。呼び出し側が固定の識別子を渡す。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
    /// `DeviceError::Cancelled` に相当する。利用者が取り消した（失敗ではない）。
    #[error("操作がキャンセルされました: eye_dropper")]
    Cancelled,
    /// `DeviceError::Unsupported` に相当する。
    #[error("このプラットフォームでは未対応です: eye_dropper")]
    Unsupported,
}

impl EyeDropperError {
    /// 契約（`fandhe-device`）が返す `DeviceError` を写す。この機能は権限を要さないため、権限拒否など
    /// 想定外の種類が来ても成功にはせず `Internal` の失敗に畳む。
    #[cfg_attr(
        not(any(target_os = "macos", target_arch = "wasm32", test)),
        allow(dead_code)
    )]
    pub(crate) fn from_device(e: DeviceError) -> Self {
        match e {
            DeviceError::Cancelled(_) => Self::Cancelled,
            DeviceError::Unsupported(_) => Self::Unsupported,
            DeviceError::Internal(s) => Self::Internal(s),
            _ => Self::Internal("eye_dropper"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        assert_eq!(
            EyeDropperError::Internal("eye_dropper").to_string(),
            "内部エラーが発生しました: eye_dropper"
        );
        assert_eq!(
            EyeDropperError::Cancelled.to_string(),
            "操作がキャンセルされました: eye_dropper"
        );
        assert_eq!(
            EyeDropperError::Unsupported.to_string(),
            "このプラットフォームでは未対応です: eye_dropper"
        );
    }

    #[test]
    fn from_device_maps_kinds_and_never_succeeds() {
        assert_eq!(
            EyeDropperError::from_device(DeviceError::Cancelled("x")),
            EyeDropperError::Cancelled
        );
        assert_eq!(
            EyeDropperError::from_device(DeviceError::Unsupported("x")),
            EyeDropperError::Unsupported
        );
        assert_eq!(
            EyeDropperError::from_device(DeviceError::Internal("y")),
            EyeDropperError::Internal("y")
        );
        assert_eq!(
            EyeDropperError::from_device(DeviceError::PermissionDenied("x")),
            EyeDropperError::Internal("eye_dropper")
        );
    }
}
