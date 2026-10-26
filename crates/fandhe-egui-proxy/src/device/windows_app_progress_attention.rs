//! Windows 向け進捗表示・注意喚起実装（Issue #860）。`ITaskbarList3::SetProgressValue` /
//! `SetProgressState(TBPF_NOPROGRESS)` でメインウィンドウのタスクバーボタンに進捗（0〜100 %）を
//! 出す／消し、注意喚起は `FlashWindowEx`（`FLASHW_ALL | FLASHW_TIMERNOFG`、前面に戻るまで点滅）。
//! HWND は `device/windows_share.rs::window()` と共通。`device/app_progress_attention.rs` の
//! Windows 経路から呼ばれ、契約は `fandhe_device::app_progress_attention`。
//!
//! HWND の有無の判定は純関数 [`plan`] にまとめ、ホストでもテストできるようモジュールは
//! `cfg(any(windows, test))`。`unsafe` は COM / `FlashWindowEx` 呼び出しに閉じ `// SAFETY:` を
//! 付ける。`ITaskbarList3` は呼び出しごとに作り、関数末尾の `Drop`（Release）で全経路で解放する。
//!
//! # 既知の天井
//!
//! - `TaskbarButtonCreated` 前に呼んだ場合の挙動は範囲外（実機確認 #852）
//! - ハイコントラスト配色では進捗が出ない（OS の仕様、事前判定しない）
//! - 複数ウィンドウのグループ表示は範囲外（デスクトップは 1 プロセス 1 画面）
//! - `HRESULT` の失敗はすべて `Internal` に写す。こちらから `CoInitializeEx` はしない
//!   （`CO_E_NOTINITIALIZED` も失敗として返す）
//! - `FlashWindowEx` の戻り値は「直前のキャプション状態」で失敗を表さないため結果に使わない

use fandhe_device::DeviceError;

/// HWND の有無の判定。無ければ OS を呼ばず `Internal`。
pub(crate) fn plan(has_window: bool) -> Result<(), DeviceError> {
    if !has_window {
        return Err(DeviceError::Internal("app_progress_attention window"));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod com {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList, TBPF_NOPROGRESS};
    use windows::Win32::UI::WindowsAndMessaging::{
        FlashWindowEx, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG,
    };

    /// `ITaskbarList3` を作って初期化する。戻り値は `Drop` で Release される（契約表 9）。
    fn taskbar() -> windows::core::Result<ITaskbarList3> {
        // SAFETY: `TaskbarList`/`CLSCTX_INPROC_SERVER` は定数のみ。失敗は `Result` で返る。
        let list: ITaskbarList3 =
            unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }?;
        // SAFETY: `list` は直前に取得した有効な COM ポインタ。
        unsafe { list.HrInit() }?;
        Ok(list)
    }

    /// 進捗（%）を設定する。値の範囲は呼び出し前に `ProgressValue` で検証済み。
    pub(super) fn set_progress(hwnd: HWND, percent: u32) -> windows::core::Result<()> {
        let list = taskbar()?;
        // SAFETY: `hwnd` は `windows_share::window()` が返したメインウィンドウ、`list` は有効な COM ポインタ。
        unsafe { list.SetProgressValue(hwnd, u64::from(percent), 100) }
    }

    /// 進捗表示を消す。
    pub(super) fn clear_progress(hwnd: HWND) -> windows::core::Result<()> {
        let list = taskbar()?;
        // SAFETY: 同上。
        unsafe { list.SetProgressState(hwnd, TBPF_NOPROGRESS) }
    }

    /// 前面に戻るまでタスクバーボタンを点滅させる。
    pub(super) fn flash(hwnd: HWND) {
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd,
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        // SAFETY: `info` は `cbSize` を正しく設定した有効な構造体で、呼び出し中生きる。
        // 戻り値は「呼ぶ前にキャプションが active 描画だったか」で失敗を表さないため捨てる。
        let _ = unsafe { FlashWindowEx(&info) };
    }
}

#[cfg(target_os = "windows")]
struct WindowsAppProgressAttention;

#[cfg(target_os = "windows")]
impl WindowsAppProgressAttention {
    /// HWND を取り、無ければ `Internal`。
    fn hwnd() -> Result<windows::Win32::Foundation::HWND, DeviceError> {
        let hwnd = super::windows_share::window();
        plan(hwnd.is_some())?;
        hwnd.ok_or(DeviceError::Internal("app_progress_attention window"))
    }
}

#[cfg(target_os = "windows")]
impl fandhe_device::app_progress_attention::AppProgressAttention for WindowsAppProgressAttention {
    fn set_progress(
        &self,
        value: fandhe_device::app_progress_attention::ProgressValue,
    ) -> Result<(), DeviceError> {
        let hwnd = Self::hwnd()?;
        com::set_progress(hwnd, value.get())
            .map_err(|_| DeviceError::Internal("app_progress_attention progress"))
    }

    fn clear_progress(&self) -> Result<(), DeviceError> {
        let hwnd = Self::hwnd()?;
        com::clear_progress(hwnd).map_err(|_| DeviceError::Internal("app_progress_attention clear"))
    }

    fn request_attention(&self) -> Result<(), DeviceError> {
        com::flash(Self::hwnd()?);
        Ok(())
    }
}

/// Windows の `AppProgressAttention` 実装を返す（`device/app_progress_attention.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation(
) -> Box<dyn fandhe_device::app_progress_attention::AppProgressAttention> {
    Box::new(WindowsAppProgressAttention)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_table() {
        assert!(plan(true).is_ok());
        assert!(matches!(
            plan(false),
            Err(DeviceError::Internal("app_progress_attention window"))
        ));
    }
}
