//! Android 向け「OS の設定画面を開く」の骨格（親 #171、Issue #798）。`device/open_system_settings.rs` の
//! Android 経路（dispatch）から呼ばれる。実体は #800 が埋めるまで `Unsupported` を返すスタブで、
//! 成功を装わない。契約の SSOT は `fandhe_device::open_system_settings`。
//!
//! # #800 への約束
//!
//! - 触ってよいのは本ファイル・Java ヘルパー・JNI だけ。dispatch・fallback 行・`device.rs`・
//!   共通コア画面・`registry.rs` は触らない
//! - 要求 id の追跡は `RequestTracker` を使う
//! - JNI の署名は 3 か所（Java 側・no-op スタブ・実体）で一致させる
//! - Activity が RESUMED でなければ起動せずコード 2 を返す
//! - `ActivityNotFoundException` はコード 1 にする。API 24・25 の通知設定もここに入る
//! - 結果コードは `device/open_system_settings.rs` の `outcome_from_android_code` に従う
//! - `resolveActivity` は呼ばない。`<queries>` と権限は足さない。頁を別の頁で代わりに開かない

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
