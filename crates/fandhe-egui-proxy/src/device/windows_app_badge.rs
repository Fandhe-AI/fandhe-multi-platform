//! Windows 向けタスクバー進捗実装（Issue #1360）。`ITaskbarList3` の `SetProgressValue` /
//! `SetProgressState` でメインウィンドウのタスクバーボタンに進捗 % を出す（0 は
//! `TBPF_NOPROGRESS` で消す）。HWND は `device/windows_share.rs::window()` と共通。
//! `device/app_badge.rs` の Windows 経路から呼ばれ、契約は `fandhe_device::app_badge`。
//!
//! 入力の検証と HWND の有無の判定は純関数 [`plan`] にまとめ、ホストでもテストできるよう
//! モジュールは `cfg(any(windows, test))`。`unsafe` は COM 呼び出しに閉じ `// SAFETY:` を付ける。
//!
//! # 既知の天井
//!
//! - `TaskbarButtonCreated` 前に呼んだ場合の挙動は範囲外（実機確認 #1359）
//! - オーバーレイアイコン（`SetOverlayIcon`）は範囲外
//! - `HRESULT` の失敗はすべて `Internal("app_badge set")` に写す。こちらから `CoInitializeEx` は
//!   しない（`CO_E_NOTINITIALIZED` も失敗として返す）

use fandhe_device::app_badge::BadgeValue;
use fandhe_device::DeviceError;

/// 実行する操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// 進捗表示を消す。
    Clear,
    /// 進捗 % を出す。
    Progress(u32),
}

/// 検証と HWND 判定。入力の検証を HWND の判定より先に置く（契約表 5）。
pub(crate) fn plan(value: BadgeValue, has_window: bool) -> Result<Step, DeviceError> {
    let step = if value.is_clear() {
        Step::Clear
    } else {
        Step::Progress(value.as_progress_percent()?)
    };
    if !has_window {
        return Err(DeviceError::Internal("app_badge window"));
    }
    Ok(step)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod com {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList, TBPF_NOPROGRESS, TBPF_NORMAL};

    use super::Step;

    /// COM 呼び出しの失敗は呼び出し元で `Internal` に写す（OS の文言は出さない）。
    pub(super) fn apply(
        hwnd: windows::Win32::Foundation::HWND,
        step: Step,
    ) -> windows::core::Result<()> {
        // SAFETY: `TaskbarList`/`CLSCTX_INPROC_SERVER` は定数のみ。失敗は `Result` で返る。
        let list: ITaskbarList3 =
            unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }?;
        // SAFETY: `list` は直前に取得した有効な COM ポインタ（`Drop` で Release される）。
        unsafe { list.HrInit() }?;
        match step {
            // SAFETY: `hwnd` は `windows_share::window()` が返したメインウィンドウ。
            Step::Clear => unsafe { list.SetProgressState(hwnd, TBPF_NOPROGRESS) },
            Step::Progress(p) => {
                // SAFETY: 同上。`p` は 0〜100 に検証済み。
                unsafe { list.SetProgressValue(hwnd, u64::from(p), 100) }?;
                // SAFETY: 同上。
                unsafe { list.SetProgressState(hwnd, TBPF_NORMAL) }
            }
        }
    }
}

#[cfg(target_os = "windows")]
struct WindowsAppBadge;

#[cfg(target_os = "windows")]
impl fandhe_device::app_badge::AppBadge for WindowsAppBadge {
    fn set(
        &self,
        value: BadgeValue,
    ) -> Result<fandhe_device::app_badge::BadgeOutcome, DeviceError> {
        let hwnd = super::windows_share::window();
        let step = plan(value, hwnd.is_some())?;
        let Some(hwnd) = hwnd else {
            return Err(DeviceError::Internal("app_badge window"));
        };
        com::apply(hwnd, step).map_err(|_| DeviceError::Internal("app_badge set"))?;
        Ok(fandhe_device::app_badge::BadgeOutcome::Submitted)
    }
}

/// Windows の `AppBadge` 実装を返す（`device/app_badge.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::app_badge::AppBadge> {
    Box::new(WindowsAppBadge)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(n: u32) -> BadgeValue {
        BadgeValue::new(n).unwrap()
    }

    #[test]
    fn plan_table() {
        assert_eq!(plan(v(0), true).unwrap(), Step::Clear);
        assert_eq!(plan(v(50), true).unwrap(), Step::Progress(50));
        assert_eq!(plan(v(100), true).unwrap(), Step::Progress(100));
        assert!(matches!(
            plan(v(101), true),
            Err(DeviceError::InvalidInput("app_badge progress"))
        ));
        assert!(matches!(
            plan(v(5), false),
            Err(DeviceError::Internal("app_badge window"))
        ));
        // 入力の検証が HWND の判定より先。
        assert!(matches!(
            plan(v(101), false),
            Err(DeviceError::InvalidInput(_))
        ));
    }
}
