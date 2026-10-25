//! iOS 向け「OS の設定画面を開く」の骨格（親 #171、Issue #798）。`device/open_system_settings.rs` の
//! iOS 経路（dispatch）から呼ばれる。実体は #799 が埋めるまで `Unsupported` を返すスタブで、
//! 成功を装わない。契約の SSOT は `fandhe_device::open_system_settings`。
//!
//! # #799 への約束
//!
//! - 触ってよいのは本ファイル・`ios/fandhe_ios_open_system_settings.m`・`build.rs` の iOS 分岐だけ。
//!   dispatch・fallback 行・`device.rs`・共通コア画面・`registry.rs` は触らない
//! - `UIApplication.open(_:options:completionHandler:)` は main queue で呼ぶ
//! - completion は 1 回だけ `bridge::deliver` へ送り、直後に `crate::ios::request_repaint` を呼ぶ
//!   （入力がないと egui が描画せず、画面が処理中のまま止まる）
//! - 結果コードは `device/open_system_settings.rs` の `outcome_from_ios_code` に従う
//! - `canOpenURL` と非公開の URL（`App-Prefs:` 等）は使わない。期限・タイマーは設けない

use fandhe_device::open_system_settings::SettingsPage;
use fandhe_device::DeviceError;

/// OS へ設定画面の表示を依頼する。`Ok` は受理済みで、結果は後から `bridge::deliver` で届く。
/// 実体が入るまでは `Unsupported`。
pub(crate) fn begin_open(_request_id: u64, _page: SettingsPage) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("open settings"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_is_unsupported_for_both_pages() {
        for p in [SettingsPage::AppSettings, SettingsPage::Notifications] {
            assert!(matches!(
                begin_open(1, p),
                Err(DeviceError::Unsupported("open settings"))
            ));
        }
    }
}
