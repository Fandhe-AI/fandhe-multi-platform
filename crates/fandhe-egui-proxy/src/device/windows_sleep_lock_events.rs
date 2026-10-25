//! Windows 向けスリープ・ロックの通知実装（Issue #810、親 #167）。`device/sleep_lock_events.rs`
//! （effect ドライバ）の windows 経路から UI スレッドで呼ばれ、契約は
//! `fandhe_device::sleep_lock_events`。ホストでは写像（`fandhe_device` の純関数）だけをテストし、
//! Win32 を呼ぶ部分は Windows ビルドだけ。
//!
//! # 方式
//!
//! - 購読ごとに専用スレッド（`fandhe-sleep-lock-events`）を 1 本立てる。スレッドは
//!   `PeekMessageW(PM_NOREMOVE)` でメッセージキューを作り、不可視のトップレベルウィンドウ
//!   （`WS_EX_TOOLWINDOW`・`WS_VISIBLE` なし）を作って、`WTSRegisterSessionNotification`
//!   （ロック・解除）と `RegisterSuspendResumeNotification`（スリープ・復帰）を登録する。
//!   `HWND_MESSAGE` は使わない（`WM_POWERBROADCAST` はトップレベルウィンドウ向けで、message-only へ
//!   届く保証が無い。`windows_system_tray.rs` と同じ判断）
//! - ウィンドウプロシージャは `event_from_windows` で出来事に写し、スレッドローカルの `id` で
//!   `fire` を呼ぶだけ（すぐ戻る）。`PBT_APMRESUMESUSPEND` は数えない（利用者の操作での復帰は
//!   `PBT_APMRESUMEAUTOMATIC` に続いて届き、二重計上になるため）
//! - 停止（[`WinGuard`] の drop）は「登録解除 → `PostThreadMessageW` で停止を依頼 → join」の順。
//!   スレッドは `UnregisterSuspendResumeNotification` → `WTSUnRegisterSessionNotification`
//!   （登録と 1 対 1）→ `DestroyWindow` を行ってループを抜ける。ローカルの Win32 呼び出しだけなので
//!   join に期限は設けない（`windows_global_hotkey.rs` と同じ理由）
//! - `GetMessageW` が -1 を返したら後片付けをしてから `Signal::Failed` を送る（実行中の失敗）
//! - 開始の途中で失敗したら、それまでの登録を逆順に解除してウィンドウを壊し、`Err` を返す
//!
//! # OS を正にする範囲
//!
//! スリープ・復帰・ロック・解除の 4 種を知らせる。画面の消灯・点灯の通知は取らない。今のロック状態は
//! `WTSQuerySessionInformationW(WTSSessionInfoEx)` の `SessionFlags` で毎回 OS へ問う。
//! 戻りは `Level` とサイズを確かめてから読み、全経路で `WTSFreeMemory` する。
//! OS のエラーコードはログにもエラーにも出さない。
//!
//! # 守らない範囲
//!
//! 共通の守備範囲は trait doc を参照。Windows 固有: 同一セッションの他プロセスが送る偽の
//! `WM_WTSSESSION_CHANGE` / `WM_POWERBROADCAST` は記録が増えるだけ（今の状態は OS へ問い直す）。
//! `SessionFlags` は Windows 7 / Server 2008 R2 で LOCK と UNLOCK が逆になる（対象外の版）。
//! 実機での挙動（unpackaged で動くか、`PBT_APMRESUMEAUTOMATIC` の到着）は #806 で確認する。

use std::sync::Arc;

use fandhe_device::sleep_lock_events::SleepLockEvents;

/// Windows の `SleepLockEvents` 実装を返す（`device/sleep_lock_events.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn SleepLockEvents> {
    Arc::new(win::WindowsSleepLockEvents)
}

mod win {
    use std::cell::Cell;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::mpsc::sync_channel;
    use std::thread::JoinHandle;

    use fandhe_device::sleep_lock_events::{
        event_from_windows, lock_state_from_session_flags, CurrentState, EventSupport, OnSignal,
        Signal, SleepLockEvents, SubscriptionGuard,
    };
    use fandhe_device::DeviceError;
    use windows::core::{w, PWSTR};
    use windows::Win32::Foundation::{
        GetLastError, ERROR_CLASS_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Power::{
        RegisterSuspendResumeNotification, UnregisterSuspendResumeNotification, HPOWERNOTIFY,
    };
    use windows::Win32::System::RemoteDesktop::{
        WTSFreeMemory, WTSQuerySessionInformationW, WTSRegisterSessionNotification,
        WTSSessionInfoEx, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION, WTSINFOEXW,
        WTS_CURRENT_SESSION,
    };
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PeekMessageW, PostThreadMessageW, RegisterClassW, TranslateMessage,
        DEVICE_NOTIFY_WINDOW_HANDLE, MSG, PM_NOREMOVE, WM_APP, WM_POWERBROADCAST,
        WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW,
    };

    use super::super::sleep_lock_events::{fire, register, unregister};

    /// `WM_APP + 1`。UI 側から専用スレッドへ停止を依頼するメッセージ。
    const WM_APP_STOP: u32 = WM_APP + 1;

    // 契約側の自前定数が windows クレートの値と一致していることをビルド時に確かめる。
    const _: () =
        assert!(WM_WTSSESSION_CHANGE == fandhe_device::sleep_lock_events::WM_WTSSESSION_CHANGE);
    const _: () = assert!(WM_POWERBROADCAST == fandhe_device::sleep_lock_events::WM_POWERBROADCAST);
    const _: () = assert!(
        windows::Win32::UI::WindowsAndMessaging::WTS_SESSION_LOCK as usize
            == fandhe_device::sleep_lock_events::WTS_SESSION_LOCK
    );
    const _: () = assert!(
        windows::Win32::UI::WindowsAndMessaging::WTS_SESSION_UNLOCK as usize
            == fandhe_device::sleep_lock_events::WTS_SESSION_UNLOCK
    );
    const _: () = assert!(
        windows::Win32::UI::WindowsAndMessaging::PBT_APMSUSPEND as usize
            == fandhe_device::sleep_lock_events::PBT_APMSUSPEND
    );
    const _: () = assert!(
        windows::Win32::UI::WindowsAndMessaging::PBT_APMRESUMEAUTOMATIC as usize
            == fandhe_device::sleep_lock_events::PBT_APMRESUMEAUTOMATIC
    );

    const START_FAILED: DeviceError = DeviceError::Internal("sleep_lock_events start");
    const READ_FAILED: DeviceError = DeviceError::Internal("sleep_lock_events read");

    thread_local! {
        /// このスレッドの購読 id（ウィンドウプロシージャが `fire` の宛先に使う）。
        static SUBSCRIPTION_ID: Cell<u64> = const { Cell::new(0) };
    }

    /// Windows の `SleepLockEvents`。状態は購読ごとの専用スレッドに持つためフィールドは無い。
    pub(super) struct WindowsSleepLockEvents;

    impl SleepLockEvents for WindowsSleepLockEvents {
        fn support(&self) -> EventSupport {
            // 順序は ALL_EVENTS: スリープ・復帰・ロック・解除は取れる、画面の消灯・点灯は取れない。
            EventSupport {
                events: [true, true, false, false, true, true],
                lock: true,
                screen: false,
            }
        }

        fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
            let id = register(on_signal);
            let (ready_tx, ready_rx) = sync_channel::<Result<u32, ()>>(1);
            let join = match std::thread::Builder::new()
                .name("fandhe-sleep-lock-events".into())
                .spawn(move || worker_main(id, ready_tx))
            {
                Ok(j) => j,
                Err(_) => {
                    unregister(id);
                    return Err(START_FAILED);
                }
            };
            // 専用スレッドはローカルの Win32 呼び出しだけで即座に返事をするため期限は設けない。
            match ready_rx.recv() {
                Ok(Ok(thread_id)) => Ok(Box::new(WinGuard {
                    id,
                    thread_id,
                    join: Some(join),
                })),
                _ => {
                    let _ = join.join();
                    unregister(id);
                    Err(START_FAILED)
                }
            }
        }

        #[allow(unsafe_code)]
        fn read_current(&self) -> Result<CurrentState, DeviceError> {
            let mut buf = PWSTR::null();
            let mut bytes = 0u32;
            // SAFETY: `buf` と `bytes` は有効なローカル変数への書き込み先。サーバーは NULL（ローカル）、
            // セッションは現在のセッション。確保された領域は下で必ず `WTSFreeMemory` する。
            let queried = unsafe {
                WTSQuerySessionInformationW(
                    Some(HANDLE::default()),
                    WTS_CURRENT_SESSION,
                    WTSSessionInfoEx,
                    &mut buf,
                    &mut bytes,
                )
            };
            if queried.is_err() || buf.is_null() {
                return Err(READ_FAILED);
            }
            let flags = if (bytes as usize) >= std::mem::size_of::<WTSINFOEXW>() {
                // SAFETY: OS が確保した `bytes` バイトの領域で、`WTSINFOEXW` 以上のサイズを確かめた。
                // アラインメントを仮定せず `read_unaligned` で読む。解放は直後に行い、読んだ値は
                // コピーなのでポインタは残らない。
                let info = unsafe { std::ptr::read_unaligned(buf.0 as *const WTSINFOEXW) };
                // 指定した情報クラスは Level 1 のみ。それ以外は読めたことにしない。
                if info.Level == 1 {
                    // SAFETY: `Level == 1` のとき共用体は `WTSInfoExLevel1` が有効（公式）。
                    Some(unsafe { info.Data.WTSInfoExLevel1.SessionFlags })
                } else {
                    None
                }
            } else {
                None
            };
            // SAFETY: `WTSQuerySessionInformationW` が成功して返した領域を 1 回だけ解放する。
            unsafe { WTSFreeMemory(buf.0 as *mut std::ffi::c_void) };
            let flags = flags.ok_or(READ_FAILED)?;
            Ok(CurrentState {
                lock: Some(lock_state_from_session_flags(flags as u32)),
                screen: None,
            })
        }
    }

    /// 実行中の購読。drop で専用スレッドを止める。
    struct WinGuard {
        id: u64,
        thread_id: u32,
        join: Option<JoinHandle<()>>,
    }

    impl SubscriptionGuard for WinGuard {}

    impl Drop for WinGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから OS の購読を止める。
            unregister(self.id);
            // SAFETY: ポインタは渡さない（wParam・lParam は 0）。宛先は自分が起こした専用スレッドの
            // id で、既に終了していれば失敗を返すだけ（その場合も join は戻る）。
            let _ =
                unsafe { PostThreadMessageW(self.thread_id, WM_APP_STOP, WPARAM(0), LPARAM(0)) };
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    /// 専用スレッドが確保した OS 資源。`release` で登録と逆順に全て解除する。
    #[derive(Default)]
    struct Resources {
        hwnd: Option<HWND>,
        session: bool,
        power: Option<HPOWERNOTIFY>,
    }

    impl Resources {
        #[allow(unsafe_code)]
        fn release(&mut self) {
            if let Some(power) = self.power.take() {
                // SAFETY: このスレッドが `RegisterSuspendResumeNotification` で得た未解除のハンドル。
                let _ = unsafe { UnregisterSuspendResumeNotification(power) };
            }
            if let Some(hwnd) = self.hwnd {
                if self.session {
                    // SAFETY: 登録時と同じ有効な HWND に対する、登録と 1 対 1 の解除。
                    let _ = unsafe { WTSUnRegisterSessionNotification(hwnd) };
                    self.session = false;
                }
                // SAFETY: このスレッドが作ったウィンドウで、他では保持していない。
                let _ = unsafe { DestroyWindow(hwnd) };
                self.hwnd = None;
            }
        }
    }

    /// ウィンドウプロシージャ。出来事に写せるものだけを `fire` し、すぐ戻る。
    #[allow(unsafe_code)]
    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if let Some(event) = event_from_windows(msg, wparam.0) {
            let id = SUBSCRIPTION_ID.with(|c| c.get());
            // 通知先の panic でメッセージループを止めない。
            let _ = catch_unwind(AssertUnwindSafe(|| fire(id, Signal::Event(event))));
        }
        if msg == WM_POWERBROADCAST {
            // 処理済みを返す（TRUE）。
            return LRESULT(1);
        }
        // SAFETY: OS から渡された引数をそのまま既定の処理へ渡す。
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// 専用スレッドの本体。キュー作成 → ウィンドウ → 登録 → 返事 → メッセージループ → 解除。
    #[allow(unsafe_code)]
    fn worker_main(id: u64, ready_tx: std::sync::mpsc::SyncSender<Result<u32, ()>>) {
        SUBSCRIPTION_ID.with(|c| c.set(id));
        let mut msg = MSG::default();
        // SAFETY: `msg` はこのスレッドのスタック上の有効な MSG。メッセージを取り出さずに呼ぶだけで、
        // `PostThreadMessageW` を受けられるキューが作られる。
        let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
        // SAFETY: 引数のない取得関数。
        let thread_id = unsafe { GetCurrentThreadId() };

        let mut res = Resources::default();
        if setup(&mut res).is_err() {
            res.release();
            let _ = ready_tx.send(Err(()));
            return;
        }
        if ready_tx.send(Ok(thread_id)).is_err() {
            // 呼び出し側が消えた。登録を残さない。
            res.release();
            return;
        }

        let mut failed = false;
        loop {
            // SAFETY: `msg` は有効な MSG。hWnd NULL・範囲 0 でこのスレッドの全メッセージを受ける。
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
            if ret < 0 {
                failed = true;
                break;
            }
            if ret == 0 || msg.message == WM_APP_STOP {
                break;
            }
            // SAFETY: `GetMessageW` が埋めた有効な MSG。
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        res.release();
        if failed {
            fire(id, Signal::Failed);
        }
    }

    /// ウィンドウの作成と 2 種類の登録。失敗したら呼び出し側が `release` で戻す。
    #[allow(unsafe_code)]
    fn setup(res: &mut Resources) -> Result<(), ()> {
        let class_name = w!("FandheSleepLockEventsWindow");
        // SAFETY: 引数 NULL で現在のモジュールのハンドルを得る。
        let instance = unsafe { GetModuleHandleW(None) }.map_err(|_| ())?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        // SAFETY: `class` は有効な WNDCLASSW。クラス名は静的な文字列で、プロシージャは `'static`。
        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            // SAFETY: 引数のない取得関数。
            if unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
                return Err(());
            }
        }
        // SAFETY: 全パラメータは固定値かローカル変数。`WS_VISIBLE` を付けないため画面に出ない。
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name,
                class_name,
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .map_err(|_| ())?;
        res.hwnd = Some(hwnd);
        // SAFETY: 有効な HWND（このスレッドが作成）。
        unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) }.map_err(|_| ())?;
        res.session = true;
        // SAFETY: ウィンドウハンドルを受信先に指定する（DEVICE_NOTIFY_WINDOW_HANDLE）。
        let power = unsafe {
            RegisterSuspendResumeNotification(HANDLE(hwnd.0), DEVICE_NOTIFY_WINDOW_HANDLE)
        }
        .map_err(|_| ())?;
        res.power = Some(power);
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::Arc;

        #[test]
        fn start_read_current_and_drop_returns() {
            let imp = WindowsSleepLockEvents;
            let guard = imp.start(Arc::new(|_| {})).expect("開始できる");
            let cur = imp.read_current().expect("読める");
            assert!(cur.lock.is_some());
            drop(guard);
        }
    }
}
