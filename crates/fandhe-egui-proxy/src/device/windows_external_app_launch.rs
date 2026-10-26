//! Windows 向け「電話発信・地図など他アプリの起動」の実装（親 #176、Issue #861）。
//! `device/external_app_launch.rs` の Windows 経路（dispatch）から呼ばれる。契約は
//! `fandhe_device::external_app_launch`。
//!
//! 電話は固定の `tel:` URI を `ShellExecuteExW`（`SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI`）で開く。
//! 失敗は `GetLastError` で「開けるアプリなし」「利用者のキャンセル」を分ける。地図は Windows の地図
//! アプリが非推奨のため OS を呼ばず `TargetUnavailable` を返す（別の URI で代わりに開かない）。
//! 渡す URI は `fandhe-device` の定数だけで、外から URI を受ける経路は無い。`windows_open_in_shell.rs` の
//! 呼び出し経路を写したもので、共通化はしない（他機能のファイルを触らないため）。
//!
//! 写像は純関数 [`uri_for`] / [`exec_failure_outcome`] に置き、ホストでもテストできるよう
//! モジュールは `cfg(any(windows, test))`。
//!
//! # 既知の天井
//!
//! - 成功は「OS が受け付けた」だけ（`HandedOff`）。`tel:` に対応するアプリ（Phone Link 等）が出たかは不明
//! - `SEE_MASK_FLAG_NO_UI` 下でも「アプリを選ぶ」が出る場合がある
//!
//! いずれも Windows 実機確認（#856）で見る。

use fandhe_device::external_app_launch::{LaunchOutcome, LaunchTarget, PHONE_URI};
use fandhe_device::DeviceError;

/// `ShellExecuteExW` の `GetLastError`: 関連付けられたアプリが無い。
const ERROR_NO_ASSOCIATION: u32 = 1155;
/// `ShellExecuteExW` の `GetLastError`: 利用者が取り消した。
const ERROR_CANCELLED: u32 = 1223;
/// `hInstApp` の `SE_ERR_NOASSOC`: 関連付けが無い。
const SE_ERR_NOASSOC: isize = 31;

/// 操作に対応する URI。地図は Windows に開く先が無いため `None`（OS を呼ばない）。
pub(crate) fn uri_for(target: LaunchTarget) -> Option<&'static str> {
    match target {
        LaunchTarget::Phone => Some(PHONE_URI),
        LaunchTarget::Map => None,
    }
}

/// `ShellExecuteExW` 失敗時の写像。`last_error` は `GetLastError`、`inst_app` は `hInstApp` の数値。
/// 関連付け無しは `NoHandler`、キャンセルは `Cancelled`、未知の失敗は `Internal`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn exec_failure_outcome(
    last_error: u32,
    inst_app: isize,
) -> Result<LaunchOutcome, DeviceError> {
    if last_error == ERROR_NO_ASSOCIATION || inst_app == SE_ERR_NOASSOC {
        Ok(LaunchOutcome::NoHandler)
    } else if last_error == ERROR_CANCELLED {
        Ok(LaunchOutcome::Cancelled)
    } else {
        Err(DeviceError::Internal("external app os"))
    }
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// COM（STA）初期化ガード。成功（`S_OK`/`S_FALSE`）時だけ Drop で解放する。
    /// `windows_open_in_shell.rs` の `ComGuard` と同型。
    struct ComGuard(bool);
    impl ComGuard {
        #[allow(unsafe_code)]
        fn new() -> Self {
            // SAFETY: 呼び出しスレッドへの初期化要求。失敗（`RPC_E_CHANGED_MODE` 等）でも
            // プロセス状態を壊さず、その場合は解放しない。
            let hr =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            Self(hr.is_ok())
        }
    }
    impl Drop for ComGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: `new` で成功した初期化と 1 対 1 に対応する。
                unsafe { CoUninitialize() };
            }
        }
    }

    #[allow(unsafe_code)]
    pub(super) fn open(uri: &str) -> Result<LaunchOutcome, DeviceError> {
        let file = HSTRING::from(uri);
        let _com = ComGuard::new();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("open"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR::null(),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // SAFETY: `info` は `cbSize` を設定した初期化済み構造体。`lpFile` は `file`（NUL 終端の UTF-16）、
        // `lpVerb` は静的な NUL 終端文字列で、いずれも呼び出しの間だけ生きる。UI スレッドから同期に呼ぶ。
        match unsafe { ShellExecuteExW(&mut info) } {
            Ok(()) => Ok(LaunchOutcome::HandedOff),
            Err(e) => {
                // `windows::core::Error` は Win32 エラーを `0x8007xxxx` の HRESULT で持つ。
                let hr = e.code().0 as u32;
                let last = if hr >> 16 == 0x8007 { hr & 0xFFFF } else { 0 };
                // `hInstApp` は失敗時に `SE_ERR_*` の数値コードが入る（ポインタとしては読まない）。
                exec_failure_outcome(last, info.hInstApp.0 as isize)
            }
        }
    }
}

#[cfg(target_os = "windows")]
struct WindowsExternalAppLaunch;

#[cfg(target_os = "windows")]
impl fandhe_device::external_app_launch::ExternalAppLaunch for WindowsExternalAppLaunch {
    fn launch(&self, target: LaunchTarget) -> Result<LaunchOutcome, DeviceError> {
        match uri_for(target) {
            None => Ok(LaunchOutcome::TargetUnavailable),
            Some(uri) => win::open(uri),
        }
    }
}

/// Windows の `ExternalAppLaunch` 実装を返す（`device/external_app_launch.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::external_app_launch::ExternalAppLaunch> {
    Box::new(WindowsExternalAppLaunch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_has_no_uri_and_phone_is_fixed() {
        assert_eq!(uri_for(LaunchTarget::Map), None);
        assert_eq!(uri_for(LaunchTarget::Phone), Some(PHONE_URI));
    }

    #[test]
    fn failure_table_never_makes_cancel_or_unknown_a_success() {
        assert_eq!(
            exec_failure_outcome(ERROR_NO_ASSOCIATION, 0).unwrap(),
            LaunchOutcome::NoHandler
        );
        assert_eq!(
            exec_failure_outcome(0, SE_ERR_NOASSOC).unwrap(),
            LaunchOutcome::NoHandler
        );
        assert_eq!(
            exec_failure_outcome(ERROR_CANCELLED, 0).unwrap(),
            LaunchOutcome::Cancelled
        );
        for (last, inst) in [(0, 0), (5, 2), (2, 0), (u32::MAX, isize::MIN)] {
            assert!(exec_failure_outcome(last, inst).is_err(), "{last} {inst}");
        }
    }
}
