//! Android 向けアプリショートカット実装のスタブ（Issue #424、親 #101）。常に `Unsupported` を返す。
//! `device/app_shortcuts.rs::platform` の Android 経路から呼ばれ、契約は
//! `fandhe_device::app_shortcuts`。
//!
//! 兄弟 issue（b-android）がこのファイルの中身だけを埋める。シグネチャと dispatch
//! （`device/app_shortcuts.rs`）は変えない。呼び出しは別スレッドで行われる（`ON_WORKER`）。
//! 埋める側の申し送り: `ShortcutManager.setDynamicShortcuts` で登録し `Registration::Installed` を返す。
//! API 25 未満は `Unsupported`、`false` の戻りは失敗にする。起動 intent は `SINGLE_TOP` と
//! `CLEAR_TOP` を考慮し、既存の url-scheme の `onNewIntent` 経路へ載せる。

use fandhe_device::app_shortcuts::{AppShortcuts, UnsupportedAppShortcuts};

/// Android の実装を返す（b-android が実体に差し替える）。
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
