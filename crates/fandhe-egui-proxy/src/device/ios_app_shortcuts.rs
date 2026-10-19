//! iOS 向けアプリショートカット実装のスタブ（Issue #424、親 #101）。常に `Unsupported` を返す。
//! `device/app_shortcuts.rs::platform` の iOS 経路から呼ばれ、契約は
//! `fandhe_device::app_shortcuts`。
//!
//! 兄弟 issue（b-ios）がこのファイルの中身だけを埋める。シグネチャと dispatch
//! （`device/app_shortcuts.rs`）は変えない。
//! 埋める側の申し送り: `Registration::Declared` を返す。Info.plist の
//! `UIApplicationShortcutItems`（静的）と、scene delegate / `application:performActionForShortcutItem:`
//! の 2 経路から、`fandhe_core::screens::url_scheme_show::SHORTCUT_URL` を `UrlInbox` へ渡す。

use fandhe_device::app_shortcuts::{AppShortcuts, UnsupportedAppShortcuts};

/// iOS の実装を返す（b-ios が実体に差し替える）。
pub(crate) fn implementation() -> &'static dyn AppShortcuts {
    &UnsupportedAppShortcuts
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_unsupported() {
        assert!(matches!(
            implementation().install(),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
