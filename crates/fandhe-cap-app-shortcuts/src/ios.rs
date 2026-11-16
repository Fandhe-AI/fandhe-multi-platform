//! iOS 向けアプリショートカット実装（Issue #425、親 #101）。
//! `crates/fandhe-cap-app-shortcuts/src/driver.rs::platform` の iOS 経路から呼ばれ、契約は
//! `fandhe_cap_app_shortcuts`。
//!
//! 登録は Info.plist の `UIApplicationShortcutItems`（静的）だけで、OS がインストール時に行う。
//! 受信は scene delegate（`ios/fandhe_ios_scene.m`）→ 通知 → `fandhe-cap-url-scheme` の `UrlInbox`
//! で、新しい受け口は持たない。表示されたかはアプリから確かめられないため `Declared` を返し、
//! `Installed` は返さない（嘘の成功を避ける）。`spec` は入力検査だけに使い、OS へは渡さない
//! （実際に出る項目は Info.plist の宣言の値で、`spec` との一致は呼び出し側が保つ）。
//! 守らない範囲: 他アプリが同じ URL を送った場合との区別、表示数の上限（OS 次第）、
//! 項目の動的な書き換え。

use crate::contract::{AppShortcuts, Registration, ShortcutSpec};
use crate::error::AppShortcutsError;

struct IosAppShortcuts;

impl AppShortcuts for IosAppShortcuts {
    fn install(&self, spec: &ShortcutSpec<'_>) -> Result<Registration, AppShortcutsError> {
        spec.validate()?;
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

    #[test]
    fn install_is_declared_only() {
        assert!(matches!(
            implementation().install(&ShortcutSpec {
                title: "t",
                url: "x://y"
            }),
            Ok(Registration::Declared)
        ));
    }
}
