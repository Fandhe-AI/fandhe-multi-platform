//! iOS 向け「電話発信・地図など他アプリの起動」（親 #176、Issue #862）。`device/external_app_launch.rs` の
//! iOS 経路（dispatch）から呼ばれ、`ios/fandhe_ios_external_app_launch.m` の
//! `UIApplication.openURL:options:completionHandler:` へ固定 URL を渡す。結果は completion から
//! [`fandhe_rust_external_app_launch_outcome`] 経由で `bridge::deliver` に届く。契約の SSOT は
//! `fandhe_device::external_app_launch`。
//!
//! - 渡す値は `PHONE_URI` / `APPLE_MAPS_URL` だけで、外から URL を受ける経路は無い
//! - `canOpenURL` と `LSApplicationQueriesSchemes` は使わない。期限・タイマーは設けない
//! - `success == NO` は `NoHandler`、依頼できなかった場合は失敗。成功に畳まない
//!
//! # 守らない範囲
//!
//! - completion が届かない場合は処理中のまま残る（期限を持たない）
//! - 電話の確認ダイアログでの取り消しは、OS が `success` で区別しないため区別できない
//! - 相手アプリが実際に出たか・発信したかは分からない（`success` は OS が受け付けた意味）

use fandhe_device::external_app_launch::{LaunchTarget, APPLE_MAPS_URL, PHONE_URI};
#[cfg(target_os = "ios")]
use fandhe_device::DeviceError;

/// 起動対象を開く固定 URL へ写す（SSOT）。
#[cfg(any(target_os = "ios", test))]
fn target_url(target: LaunchTarget) -> &'static str {
    match target {
        LaunchTarget::Phone => PHONE_URI,
        LaunchTarget::Map => APPLE_MAPS_URL,
    }
}

// `ios/fandhe_ios_external_app_launch.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_external_app_launch_open(request_id: u64, url: *const std::ffi::c_char) -> i32;
}

/// OS へ起動を依頼する。`Ok` は受理済みで、結果は後から `bridge::deliver` で届く。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub(crate) fn begin_launch(request_id: u64, target: LaunchTarget) -> Result<(), DeviceError> {
    let Ok(url) = std::ffi::CString::new(target_url(target)) else {
        return Err(DeviceError::Internal("external app present"));
    };
    // SAFETY: `url` は呼び出し中だけ生きる NUL 終端文字列で、`.m` 側は同期で NSURL へコピーし
    // ポインタを保持しない。結果は `fandhe_rust_external_app_launch_outcome` でちょうど 1 回届く。
    let code = unsafe { fandhe_ios_external_app_launch_open(request_id, url.as_ptr()) };
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("external app present"))
    }
}

/// `.m` の completion（main queue 上）から呼ばれる。結果を置いた直後に次フレームを起こす
/// （入力が無いと egui が描画せず画面が処理中のまま止まるため、契約表 4）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_external_app_launch_outcome(request_id: u64, code: i32) {
    super::external_app_launch::bridge::deliver(
        request_id,
        super::external_app_launch::outcome_from_ios_code(code),
    );
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_url_maps_each_target_to_its_fixed_url() {
        assert_eq!(target_url(LaunchTarget::Phone), PHONE_URI);
        assert_eq!(target_url(LaunchTarget::Map), APPLE_MAPS_URL);
        assert_ne!(PHONE_URI, APPLE_MAPS_URL);
    }
}
