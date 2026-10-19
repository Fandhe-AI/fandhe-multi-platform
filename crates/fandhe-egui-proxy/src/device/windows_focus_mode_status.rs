//! Windows 向け集中モードの状態実装（Issue #438、親 #104）。
//! `device/focus_mode_status.rs::implementation` の Windows 経路から呼ばれ、WinRT
//! `FocusSessionManager`（Windows 11 22H2+）を UI スレッドで同期に読む
//! （1 秒に 1 回。`IsFocusActiveChanged` は購読しない）。
//!
//! 失敗の写像（全経路で一貫）:
//! - `IsSupported()` が false、またはクラス未登録（`REGDB_E_CLASSNOTREG`。古い OS）→ `Unsupported`
//! - それ以外の失敗（`IsSupported` / `GetDefault` / `IsFocusActive`）→ `Internal`
//!
//! `unsafe` と `RoInitialize` は使わない。HRESULT と状態の値はログにも画面にも出さない
//! （security.md A09）。既知の天井: unpackaged exe での動作は実機未確認（#435）。

use fandhe_device::focus_mode_status::FocusStatusSource;
use fandhe_device::DeviceError;
use windows::Win32::Foundation::REGDB_E_CLASSNOTREG;
use windows::UI::Shell::FocusSessionManager;

/// [`FocusStatusSource`] の Windows 実装。状態を持たず、呼び出しごとに OS を読む。
pub(crate) struct WindowsFocusStatus;

const INTERNAL: DeviceError = DeviceError::Internal("focus_mode_status read");
const UNSUPPORTED: DeviceError = DeviceError::Unsupported("focus_mode_status");

impl FocusStatusSource for WindowsFocusStatus {
    fn read(&self) -> Result<bool, DeviceError> {
        match FocusSessionManager::IsSupported() {
            Ok(true) => {}
            Ok(false) => return Err(UNSUPPORTED),
            Err(e) if e.code() == REGDB_E_CLASSNOTREG => return Err(UNSUPPORTED),
            Err(_) => return Err(INTERNAL),
        }
        FocusSessionManager::GetDefault()
            .and_then(|m| m.IsFocusActive())
            .map_err(|_| INTERNAL)
    }
}
