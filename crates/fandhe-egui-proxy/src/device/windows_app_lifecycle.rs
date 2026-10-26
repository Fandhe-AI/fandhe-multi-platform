//! Windows 向けアプリの前面・背面の通知実装（Issue #893、契約は `fandhe_device::app_lifecycle`）。
//! `device/app_lifecycle.rs` の dispatch から UI スレッドで呼ばれる。
//!
//! メインウィンドウ（`windows_share::window()`）へ `SetWindowSubclass` で観測専用のサブクラスを付け、
//! `WM_ACTIVATEAPP`（操作の対象になった・外れた）と `WM_SIZE`（最小化・最小化からの復帰）を
//! `fandhe_device::app_lifecycle::event_from_windows` で出来事にする（`windows_pen_stylus.rs` と同じ手法）。
//! サブクラスは常に `DefSubclassProc` へ流し、メッセージを消費しない。refdata は購読 `id` で、
//! 生ポインタは持たない。
//!
//! # OS を正にする範囲
//!
//! デスクトップ（アンパッケージ）に凍結の概念は無く、前面・背面の 4 種だけ知らせる（`support()` で
//! 凍結 3 種は false）。今の状態は届くたびに OS へ問い直す。
//!
//! - 操作の対象か: `GetForegroundWindow` の所有プロセスが自プロセスか。前面ウィンドウが無ければ不明
//! - 見えているか: メインウィンドウが最小化（`IsIconic`）でないか。HWND が取れなければ `Err`
//!
//! `Shown` は最小化からの復帰だけ（他のウィンドウに隠れただけでは出ない。OS 間の定義の差として守らない）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗でサブクラスと登録表を残さない。停止後に届いたメッセージは登録表で捨てる。
//!   ウィンドウの破棄（`WM_NCDESTROY`）は実行中の失敗として知らせる
//! - 守らない範囲: 複数ウィンドウ、同一セッションの他プロセスが送る偽のメッセージ
//!   （記録が増えるだけで、今の状態は OS へ問い直す）
// ponytail: `WAS_MINIMIZED` と `INSTALLED_ID` はプロセス内 1 購読前提の static（`ACTIVE` が 1 件のため）。
// 複数窓・複数購読に広げるときは購読ごとの状態へ移す。

use std::sync::Arc;

#[cfg(not(target_os = "windows"))]
use fandhe_device::app_lifecycle::UnsupportedAppLifecycle;
use fandhe_device::app_lifecycle::{ActiveState, AppLifecycle};

/// 前面ウィンドウの所有プロセスから「操作の対象か」を決める。前面ウィンドウが無い（`None`）ときは
/// 不明で、前面・背面のどちらへも畳まない。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn classify_foreground(foreground_pid: Option<u32>, own_pid: u32) -> ActiveState {
    match foreground_pid {
        None => ActiveState::Unknown,
        Some(pid) if pid == own_pid => ActiveState::Active,
        Some(_) => ActiveState::Inactive,
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use fandhe_device::app_lifecycle::{
        event_from_windows, CurrentState, EventSupport, OnSignal, Signal, SubscriptionGuard,
        VisibilityState, SIZE_MAXIMIZED, SIZE_MINIMIZED, SIZE_RESTORED, WM_ACTIVATEAPP, WM_SIZE,
    };
    use fandhe_device::DeviceError;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging as wm;

    use super::super::app_lifecycle::{fire, register, unregister};

    // 手書きの定数が windows クレートの値と一致することをビルド時に確かめる。
    const _: () = {
        assert!(WM_ACTIVATEAPP == wm::WM_ACTIVATEAPP);
        assert!(WM_SIZE == wm::WM_SIZE);
        assert!(SIZE_RESTORED == wm::SIZE_RESTORED as usize);
        assert!(SIZE_MINIMIZED == wm::SIZE_MINIMIZED as usize);
        assert!(SIZE_MAXIMIZED == wm::SIZE_MAXIMIZED as usize);
    };

    const SUBCLASS_ID: usize = 0x4150_4C31; // "APL1"

    /// 直前の `WM_SIZE` までに最小化されていたか（`Shown` を最小化からの復帰だけにするため）。
    static WAS_MINIMIZED: AtomicBool = AtomicBool::new(false);
    /// いまサブクラスを付けている購読 `id`（0 は付いていない）。
    static INSTALLED_ID: AtomicU64 = AtomicU64::new(0);

    /// ウィンドウが UI スレッドのものか。サブクラスは所有スレッドからしか付け外しできない。
    #[allow(unsafe_code)]
    fn on_window_thread(hwnd: HWND) -> bool {
        // SAFETY: `hwnd` は `windows_share::window()` が返したメインウィンドウ。引数は値渡し。
        let owner = unsafe { wm::GetWindowThreadProcessId(hwnd, None) };
        // SAFETY: 引数なしの読み取り専用呼び出し。
        owner == unsafe { GetCurrentThreadId() }
    }

    /// ウィンドウ破棄。付けた覚えのある購読のときだけ外して実行中の失敗を知らせる
    /// （停止で外れた後は何もしない）。
    #[allow(unsafe_code)]
    fn on_destroy(hwnd: HWND, id: u64) {
        if INSTALLED_ID
            .compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // SAFETY: 付けたときと同じ HWND・ID。`WM_NCDESTROY` の処理中に外すのは公式の手順。
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            fire(id, Signal::Failed);
        }
    }

    /// 観測専用のサブクラスプロシージャ。常に `DefSubclassProc` へ流し、メッセージを消費しない。
    #[allow(unsafe_code)]
    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        ref_data: usize,
    ) -> LRESULT {
        match msg {
            WM_ACTIVATEAPP | WM_SIZE => {
                let (event, now) =
                    event_from_windows(msg, wparam.0, WAS_MINIMIZED.load(Ordering::Acquire));
                WAS_MINIMIZED.store(now, Ordering::Release);
                if let Some(event) = event {
                    fire(ref_data as u64, Signal::Event(event));
                }
            }
            wm::WM_NCDESTROY => on_destroy(hwnd, ref_data as u64),
            _ => {}
        }
        // SAFETY: このプロシージャが受け取った引数をそのまま次へ渡す。
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    /// 実行中の購読。drop で停止する。HWND は `Send` でないため `isize` で持つ。
    struct WinGuard {
        id: u64,
        hwnd: isize,
    }

    impl SubscriptionGuard for WinGuard {}

    impl Drop for WinGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // 先に登録を外し、以後の遅れたメッセージを捨てさせてからサブクラスを外す。
            unregister(self.id);
            // `WM_NCDESTROY` で外れた後に二重に外さないよう、自分の購読のときだけ外す。
            if INSTALLED_ID
                .compare_exchange(self.id, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let hwnd = HWND(self.hwnd as *mut std::ffi::c_void);
                // SAFETY: 付けたときと同じ HWND・ID。所有スレッド以外から呼ばれた場合は OS が失敗を返す
                // だけで、サブクラスは残るが登録表に `id` が無いため通知は捨てられる。
                let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            }
        }
    }

    pub(super) struct WindowsAppLifecycle;

    impl AppLifecycle for WindowsAppLifecycle {
        fn support(&self) -> EventSupport {
            // 順序は ALL_EVENTS: 前面・背面の 4 種は取れる。凍結 3 種は取れない。
            EventSupport {
                events: [true, true, true, true, false, false, false],
                active: true,
                visible: true,
            }
        }

        #[allow(unsafe_code)]
        fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
            let err = || DeviceError::Internal("app_lifecycle start");
            let hwnd = super::super::windows_share::window().ok_or_else(err)?;
            if !on_window_thread(hwnd) || INSTALLED_ID.load(Ordering::Acquire) != 0 {
                return Err(err());
            }
            let id = register(on_signal);
            // SAFETY: `hwnd` は有効なメインウィンドウ。引数は値渡し。
            WAS_MINIMIZED.store(unsafe { wm::IsIconic(hwnd) }.as_bool(), Ordering::Release);
            INSTALLED_ID.store(id, Ordering::Release);
            // SAFETY: 同一スレッドの有効な HWND に、購読 `id` だけを持つ（`dwRefData`）観測専用の
            // プロシージャを付ける。外すのは停止か `WM_NCDESTROY` で行い、ダングリングを残さない。
            let ok =
                unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, id as usize) };
            if !ok.as_bool() {
                INSTALLED_ID.store(0, Ordering::Release);
                unregister(id);
                return Err(err());
            }
            Ok(Box::new(WinGuard {
                id,
                hwnd: hwnd.0 as isize,
            }))
        }

        #[allow(unsafe_code)]
        fn read_current(&self) -> Result<CurrentState, DeviceError> {
            let err = || DeviceError::Internal("app_lifecycle read");
            let hwnd = super::super::windows_share::window().ok_or_else(err)?;
            // SAFETY: 引数なしの読み取り専用呼び出し。
            let fg = unsafe { wm::GetForegroundWindow() };
            let fg_pid = if fg.0.is_null() {
                None
            } else {
                let mut pid = 0u32;
                // SAFETY: `fg` は直前に取得したハンドル、出力先はローカル変数。
                let thread = unsafe { wm::GetWindowThreadProcessId(fg, Some(&mut pid)) };
                // 取得の失敗（0）は前面なしと同じ扱いにせず、読み取りの失敗にする。
                if thread == 0 || pid == 0 {
                    return Err(err());
                }
                Some(pid)
            };
            // SAFETY: 引数なしの読み取り専用呼び出し。
            let own = unsafe { GetCurrentProcessId() };
            // SAFETY: `hwnd` は有効なメインウィンドウ。引数は値渡し。
            let iconic = unsafe { wm::IsIconic(hwnd) }.as_bool();
            Ok(CurrentState {
                active: Some(classify_foreground(fg_pid, own)),
                visible: Some(if iconic {
                    VisibilityState::Hidden
                } else {
                    VisibilityState::Visible
                }),
            })
        }
    }
}

/// この OS の実装。Windows 以外（ホストのテスト）は非対応を返す。
pub(crate) fn implementation() -> Arc<dyn AppLifecycle> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(imp::WindowsAppLifecycle)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Arc::new(UnsupportedAppLifecycle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_owner_decides_active() {
        assert_eq!(classify_foreground(Some(10), 10), ActiveState::Active);
        assert_eq!(classify_foreground(Some(11), 10), ActiveState::Inactive);
    }

    #[test]
    fn no_foreground_window_is_unknown_not_inactive() {
        assert_eq!(classify_foreground(None, 10), ActiveState::Unknown);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn host_implementation_never_succeeds() {
        let imp = implementation();
        assert!(imp.start(Arc::new(|_| {})).is_err());
        assert!(imp.read_current().is_err());
    }
}
