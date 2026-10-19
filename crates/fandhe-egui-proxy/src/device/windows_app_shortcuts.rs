//! Windows 向けアプリショートカット実装のスタブ（Issue #424、親 #101）。常に `Unsupported` を返す。
//! `device/app_shortcuts.rs::platform` の Windows 経路から呼ばれ、契約は
//! `fandhe_device::app_shortcuts`。
//!
//! 兄弟 issue（windows）がこのファイルの中身だけを埋める。シグネチャと dispatch
//! （`device/app_shortcuts.rs`）は変えない。呼び出しは別スレッドで行われる（`ON_WORKER`）。
//! 埋める側の申し送り: `ICustomDestinationList`（ジャンプリスト）で登録し `Registration::Installed`
//! を返す。途中で失敗したら `AbortList` で後始末する。`windows` クレートの feature
//! `Win32_UI_Shell_Common` は (a) で `Cargo.toml` に追加済み。

use fandhe_device::app_shortcuts::{AppShortcuts, UnsupportedAppShortcuts};

/// Windows の実装を返す（windows の子が実体に差し替える）。
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
