//! Android 向けディープリンク（App Links）受信のスタブ（Issue #1278）。
//! `device/deep_link.rs::platform_deep_link` の Android 経路から呼ばれる。
//!
//! 現時点は [`UnsupportedDeepLink`] を返すだけで、成功を装わない。実体は Issue #1279 が
//! **本ファイルだけ**を埋める。埋める範囲:
//!
//! - static の `UrlInbox`
//! - JNI の受け口 1 本（署名は Java・Rust・no-op スタブ `crate::android` の 3 か所で一致させる）
//! - 許可ホストの取得（出どころは #1279 が決め、取れなければ全件拒否にする）
//! - `MainActivity.deliverUrlIntent` での https とカスタム scheme の振り分け
//! - manifest の intent-filter（`cargo-fandhe gen` から出す）

use fandhe_device::deep_link::{DeepLink, UnsupportedDeepLink};

/// Android の実装を返す（現時点は非対応固定）。
pub(crate) fn implementation() -> &'static dyn DeepLink {
    &UnsupportedDeepLink
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_never_succeeds() {
        assert!(matches!(
            implementation().snapshot(),
            Err(DeviceError::Unsupported("deep_link"))
        ));
    }
}
