//! iOS 向け「OS の設定画面を開く」実装（親 #171、Issue #799）。
//! `UIApplication.open(_:options:completionHandler:)`（`ios/fandhe_ios_open_system_settings.m`、
//! `build.rs` が iOS 分岐の feature `device` 有効時にコンパイル）を `unsafe extern "C"` 1 関数で呼ぶ。
//! `device/open_system_settings.rs` の iOS 経路（`begin_open`）から呼ばれ、dispatch・共通コア画面は
//! #798 で確定済みで触らない。契約の SSOT は `fandhe_device::open_system_settings`。
//!
//! 結果は `.m` の completion から [`fandhe_rust_open_system_settings_outcome`] が
//! `open_system_settings::bridge` へ置く（pull 型）。結果コード表の SSOT は
//! `open_system_settings::outcome_from_ios_code`。
//!
//! - 開く頁は固定 2 種。頁コードの写像は [`page_code`] が SSOT（`.m` は 0 / 1 だけを受ける）
//! - `canOpenURL` と非公開の URL（`App-Prefs:` 等）は使わない。期限・タイマーは設けない
//! - 守らない範囲: completion が届かない場合は処理中のまま残る。設定画面で利用者が何をしたかは分からない

#[cfg(target_os = "ios")]
use fandhe_device::open_system_settings::SettingsPage;
#[cfg(target_os = "ios")]
use fandhe_device::DeviceError;

// `ios/fandhe_ios_open_system_settings.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_open_system_settings_open(request_id: u64, page: i32) -> i32;
}

/// OS へ設定画面の表示を依頼する。`Ok` は受理済みで、結果は後から `bridge::deliver` で届く。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub(crate) fn begin_open(request_id: u64, page: SettingsPage) -> Result<(), DeviceError> {
    // SAFETY: 引数は値渡しの整数のみでポインタを経由しない。`.m` 側は main queue へ渡して
    // 即 return し、結果は `fandhe_rust_open_system_settings_outcome` でちょうど 1 回届く。
    let code = unsafe { fandhe_ios_open_system_settings_open(request_id, page_code(page)) };
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("open settings present"))
    }
}

/// `.m` の completion（main queue 上）から呼ばれる。結果を置いた直後に次フレームを起こす
/// （入力が無いと egui が描画せず画面が処理中のまま止まるため、契約表 4）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_open_system_settings_outcome(request_id: u64, code: i32) {
    super::open_system_settings::bridge::deliver(
        request_id,
        super::open_system_settings::outcome_from_ios_code(code),
    );
    crate::ios::request_repaint();
}

/// 頁を `.m` へ渡す整数へ写す（SSOT）。0 = 自アプリの設定、1 = 通知の設定。
#[cfg(any(target_os = "ios", test))]
fn page_code(page: fandhe_device::open_system_settings::SettingsPage) -> i32 {
    use fandhe_device::open_system_settings::SettingsPage;
    match page {
        SettingsPage::AppSettings => 0,
        SettingsPage::Notifications => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::open_system_settings::SettingsPage;

    #[test]
    fn page_code_maps_each_page_to_distinct_code() {
        assert_eq!(page_code(SettingsPage::AppSettings), 0);
        assert_eq!(page_code(SettingsPage::Notifications), 1);
    }
}
