//! iOS 向けアプリショートカット実装（Issue #425、親 #101）。
//! `device/app_shortcuts.rs::platform` の iOS 経路から呼ばれ、契約は
//! `fandhe_device::app_shortcuts`。
//!
//! 登録は Info.plist の `UIApplicationShortcutItems`（静的）だけで、OS がインストール時に行う。
//! 受信は scene delegate（`ios/fandhe_ios_scene.m`）→ 通知 → `apple_url_scheme` の `UrlInbox`
//! で、新しい受け口は持たない。表示されたかはアプリから確かめられないため `Declared` を返し、
//! `Installed` は返さない（嘘の成功を避ける）。
//! 守らない範囲: 他アプリが同じ URL を送った場合との区別、表示数の上限（OS 次第）、
//! 項目の動的な書き換え。

use fandhe_device::app_shortcuts::{AppShortcuts, Registration};
use fandhe_device::DeviceError;

struct IosAppShortcuts;

impl AppShortcuts for IosAppShortcuts {
    fn install(&self) -> Result<Registration, DeviceError> {
        Ok(Registration::Declared)
    }
}

/// iOS の実装を返す。
pub(crate) fn implementation() -> &'static dyn AppShortcuts {
    &IosAppShortcuts
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::url_scheme_show::{SHORTCUT_TITLE, SHORTCUT_URL};

    #[test]
    fn install_is_declared_only() {
        assert!(matches!(
            implementation().install(),
            Ok(Registration::Declared)
        ));
    }

    /// Info.plist の静的項目が画面の定数とずれていないこと。
    #[test]
    fn info_plist_matches_screen_constants() {
        let plist = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../platforms/apple/FandheIosApp/Resources/Info.plist"
        ));
        assert!(plist.contains("<key>UIApplicationShortcutItems</key>"));
        assert!(plist.contains(&format!("<string>{SHORTCUT_URL}</string>")));
        assert!(plist.contains(&format!("<string>{SHORTCUT_TITLE}</string>")));
    }
}
