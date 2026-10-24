//! Windows 向け「OS の設定画面を開く」の実装（親 #171、Issue #798）。`device/open_system_settings.rs` の
//! Windows 経路（dispatch）から呼ばれる。契約は `fandhe_device::open_system_settings`。
//!
//! 通知は固定の `ms-settings:notifications` を `ShellExecuteW`（`windows_compose_mail_sms::open_url`
//! を再利用）で開き、戻り値をそのまま写す。自アプリの設定（`ms-settings:appsfeatures-app?<PFN>`）は
//! パッケージ ファミリー名が要り、パッケージ化していない本アプリには無いため、OS を呼ばずに
//! `PageUnavailable` を返す（別の頁で代わりに開かない）。渡す URI は定数 1 つだけで、外から URI を
//! 受ける経路は作らない。新しい `unsafe` も、`windows` クレートの feature も足さない。
//!
//! 写像は純関数 [`uri_for`] / [`outcome_from_shell`] に置き、ホストでもテストできるよう
//! モジュールは `cfg(any(windows, test))`。
//!
//! # 既知の天井
//!
//! - 戻り値 `> 32` は「OS が受け付けた」だけで、設定画面が出たかは不明（`HandedOff`）
//! - `ms-settings:` の頁の有無は Windows の版・SKU で変わる（公式）
//! - 関連付けが無くても「アプリを選ぶ」が出て `> 32` になりうる
//!
//! いずれも Windows 実機確認（#795）で見る。

use fandhe_device::open_system_settings::{OpenSettingsOutcome, SettingsPage};
use fandhe_device::DeviceError;

use super::windows_compose_mail_sms::SE_ERR_NOASSOC;

/// 通知の設定の URI。固定値で、外から URI を受ける経路は作らない。
const NOTIFICATIONS_URI: &str = "ms-settings:notifications";

/// 頁に対応する URI。自アプリの設定は Windows に入口が無いため `None`。
pub(crate) fn uri_for(page: SettingsPage) -> Option<&'static str> {
    match page {
        SettingsPage::AppSettings => None,
        SettingsPage::Notifications => Some(NOTIFICATIONS_URI),
    }
}

/// `ShellExecuteW` の戻り値 → 結果の写像。`> 32` が受理、31 が関連付け無し、それ以外は失敗。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn outcome_from_shell(code: isize) -> Result<OpenSettingsOutcome, DeviceError> {
    match code {
        c if c > 32 => Ok(OpenSettingsOutcome::HandedOff),
        SE_ERR_NOASSOC => Ok(OpenSettingsOutcome::NotOpened),
        _ => Err(DeviceError::Internal("open settings shell")),
    }
}

#[cfg(target_os = "windows")]
struct WindowsOpenSystemSettings;

#[cfg(target_os = "windows")]
impl fandhe_device::open_system_settings::OpenSystemSettings for WindowsOpenSystemSettings {
    fn open(&self, page: SettingsPage) -> Result<OpenSettingsOutcome, DeviceError> {
        match uri_for(page) {
            None => Ok(OpenSettingsOutcome::PageUnavailable),
            Some(uri) => outcome_from_shell(super::windows_compose_mail_sms::open_url(uri)),
        }
    }
}

/// Windows の `OpenSystemSettings` 実装を返す（`device/open_system_settings.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::open_system_settings::OpenSystemSettings> {
    Box::new(WindowsOpenSystemSettings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_settings_has_no_uri_and_notifications_is_fixed() {
        assert_eq!(uri_for(SettingsPage::AppSettings), None);
        assert_eq!(
            uri_for(SettingsPage::Notifications),
            Some("ms-settings:notifications")
        );
    }

    #[test]
    fn shell_code_table() {
        assert_eq!(
            outcome_from_shell(33).unwrap(),
            OpenSettingsOutcome::HandedOff
        );
        assert_eq!(
            outcome_from_shell(31).unwrap(),
            OpenSettingsOutcome::NotOpened
        );
        for c in [0, 2, 5, 32, -1, isize::MIN] {
            assert!(outcome_from_shell(c).is_err(), "{c}");
        }
    }
}
