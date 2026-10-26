//! macOS 向け「電話発信・地図など他アプリの起動」の実装（親 #176、Issue #861）。
//! `macos/fandhe_macos_external_app_launch.m`（`NSWorkspace openURL:`、`build.rs` が macOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに
//! 閉じ `// SAFETY:` で契約を明記する。`device/external_app_launch.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::external_app_launch`。渡す URL は `fandhe-device` の固定値（`PHONE_URI` /
//! `APPLE_MAPS_URL`）だけで、外から URL を受ける経路は無い。
//!
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 成功は `HandedOff`（OS が URL を受理した）に留める。FaceTime / 地図が開いたか・発信したかは不明
//! - `tel:` を受けるアプリ（FaceTime・連携 iPhone）の有無は Mac 目視で確認する（#856）

use fandhe_device::external_app_launch::LaunchOutcome;
use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = 受理 / 1 = 扱うアプリ無し（`openURL:` が NO）/ 2 = NSURL を作れない・main thread 外。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn outcome_from_code(code: i32) -> Result<LaunchOutcome, DeviceError> {
    match code {
        0 => Ok(LaunchOutcome::HandedOff),
        1 => Ok(LaunchOutcome::NoHandler),
        2 => Err(DeviceError::Internal("external app present")),
        _ => Err(DeviceError::Internal("external app outcome")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::CString;

    // SAFETY: `macos/fandhe_macos_external_app_launch.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_external_app_open_url(url: *const std::ffi::c_char) -> i32;
    }

    /// 結果コードを返す。`CString` を作れなければ 2（NSURL を作れないのと同じ扱い）。
    #[allow(unsafe_code)]
    pub(super) fn open_url(url: &str) -> i32 {
        let Ok(c) = CString::new(url) else {
            return 2;
        };
        // SAFETY: `c` はこの呼び出しの間だけ生きる NUL 終端文字列。`.m` はポインタを呼び出し中に
        // だけ読み、保持しない。
        unsafe { fandhe_macos_external_app_open_url(c.as_ptr()) }
    }
}

#[cfg(target_os = "macos")]
struct MacExternalAppLaunch;

#[cfg(target_os = "macos")]
impl fandhe_device::external_app_launch::ExternalAppLaunch for MacExternalAppLaunch {
    fn launch(
        &self,
        target: fandhe_device::external_app_launch::LaunchTarget,
    ) -> Result<LaunchOutcome, DeviceError> {
        use fandhe_device::external_app_launch::{LaunchTarget, APPLE_MAPS_URL, PHONE_URI};
        let url = match target {
            LaunchTarget::Phone => PHONE_URI,
            LaunchTarget::Map => APPLE_MAPS_URL,
        };
        outcome_from_code(ffi::open_url(url))
    }
}

/// macOS の `ExternalAppLaunch` 実装を返す（`device/external_app_launch.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::external_app_launch::ExternalAppLaunch> {
    Box::new(MacExternalAppLaunch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert_eq!(outcome_from_code(0).unwrap(), LaunchOutcome::HandedOff);
        assert_eq!(outcome_from_code(1).unwrap(), LaunchOutcome::NoHandler);
        for code in [2, 3, -1, i32::MAX] {
            assert!(outcome_from_code(code).is_err());
        }
    }
}
