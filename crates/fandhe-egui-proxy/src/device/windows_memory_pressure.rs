//! Windows 向けメモリ逼迫の通知実装（Issue #911、契約は `fandhe_device::memory_pressure`）。
//! `device/memory_pressure.rs` の dispatch から UI スレッドで呼ばれる。Win32 の
//! `CreateMemoryResourceNotification`（`LowMemoryResourceNotification` / `HighMemoryResourceNotification`）
//! と待機で受け、COM は使わない。権限は不要。
//!
//! # 方式
//!
//! 購読 1 件につき専用スレッド 1 本が、停止イベントと Low / High の通知ハンドルを
//! `WaitForMultipleObjects(INFINITE)` で待つ（タイマー・ポーリングは使わない）。Low / High は
//! 「条件が成り立つ間 signaled」の状態オブジェクトなので、成り立っている側を待つと即時に戻り続ける。
//! このため毎回 `QueryMemoryResourceNotification` で両方を OS へ問い直し、
//! `fandhe_device::memory_pressure::windows_wait_plan` に従って反対側だけを待つ。
//! 停止（[`WinGuard`] の drop）は「登録解除 → 停止イベント → スレッド join → 停止イベントを閉じる」の順で、
//! 通知ハンドルはワーカーが終了時に閉じる。
//!
//! # 写像
//!
//! 起きた側（Low / High）を合図にする。待機の失敗・問い直しの失敗・規定外の戻り値は
//! `Signal::Failed`（実行中の失敗）。開始失敗は `Internal("memory_pressure start")`。
//! 今の状態は呼ぶたびに 2 ハンドルを作って問い、閉じる（アプリ内に覚えない）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始途中のどの失敗でも作ったハンドル・スレッド・登録表を残さない。drop が戻った後に届いた
//!   合図は登録表で捨てる。パス・OS エラーコードをログ・エラーに出さない
//! - 守らない範囲: Low → 中間 → Low のように中間帯をまたぐ変化、システム全体の値でありプロセス単位でない
//!   こと、通知の遅れ

use std::sync::Arc;

use fandhe_device::memory_pressure::MemoryPressure;
#[cfg(not(target_os = "windows"))]
use fandhe_device::memory_pressure::UnsupportedMemoryPressure;

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::sync::mpsc::sync_channel;
    use std::thread::JoinHandle;

    use fandhe_device::memory_pressure::{
        windows_level, windows_wait_plan, EventSupport, MemoryLevel, OnSignal, PressureEvent,
        Signal, SubscriptionGuard,
    };
    use fandhe_device::DeviceError;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Memory::{
        CreateMemoryResourceNotification, HighMemoryResourceNotification,
        LowMemoryResourceNotification, QueryMemoryResourceNotification,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE,
    };

    use super::super::memory_pressure::{fire, register, unregister};

    const START_FAILED: DeviceError = DeviceError::Internal("memory_pressure start");
    const READ_FAILED: DeviceError = DeviceError::Internal("memory_pressure read");

    /// Low / High の通知ハンドルの組。drop で閉じる。
    struct Notifications {
        low: HANDLE,
        high: HANDLE,
    }

    // SAFETY: 2 つの HANDLE は本構造体が一意に所有するカーネルの通知オブジェクトで、Query / Wait /
    // CloseHandle はスレッドセーフ。start で作ってワーカーへ move し、以後はワーカーだけが触って閉じる。
    #[allow(unsafe_code)]
    unsafe impl Send for Notifications {}

    impl Notifications {
        #[allow(unsafe_code)]
        fn create() -> Option<Self> {
            // SAFETY: 引数は列挙値のみ。戻り値の HANDLE は本構造体が所有し Drop で閉じる。
            let low =
                unsafe { CreateMemoryResourceNotification(LowMemoryResourceNotification) }.ok()?;
            // SAFETY: 同上。失敗時は直前に作った `low` を閉じてから返す。
            let Ok(high) =
                (unsafe { CreateMemoryResourceNotification(HighMemoryResourceNotification) })
            else {
                // SAFETY: 直前に作った有効な HANDLE で、他に使う者はいない。
                let _ = unsafe { CloseHandle(low) };
                return None;
            };
            Some(Self { low, high })
        }

        /// 両方の signaled 状態を OS へ問う。どちらかの失敗は `Err`（状態を捏造しない）。
        #[allow(unsafe_code)]
        fn level(&self) -> Result<MemoryLevel, DeviceError> {
            let mut low = BOOL(0);
            let mut high = BOOL(0);
            // SAFETY: ハンドルは本構造体が所有する有効な通知オブジェクト、出力先はローカル変数。
            unsafe {
                QueryMemoryResourceNotification(self.low, &mut low).map_err(|_| READ_FAILED)?;
                QueryMemoryResourceNotification(self.high, &mut high).map_err(|_| READ_FAILED)?;
            }
            windows_level(low.as_bool(), high.as_bool())
        }
    }

    impl Drop for Notifications {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: どちらも本構造体が所有する有効な HANDLE で、二重に閉じない。
            unsafe {
                let _ = CloseHandle(self.low);
                let _ = CloseHandle(self.high);
            }
        }
    }

    /// 待機スレッド本体。所有する通知ハンドルは終了時に閉じる。`stop` は親が所有する停止イベント
    /// （ここでは閉じない）。
    #[allow(unsafe_code)]
    fn run(id: u64, notif: Notifications, stop: usize) {
        let stop = HANDLE(stop as *mut _);
        loop {
            // OS を正にする: 待つ側は毎回の問い直しから決め、アプリ内の記憶から作らない。
            let level = match notif.level() {
                Ok(l) => l,
                Err(_) => {
                    fire(id, Signal::Failed);
                    return;
                }
            };
            let (wait_low, wait_high) = windows_wait_plan(level);
            // 停止を先頭に置き、同時なら停止を優先する。
            let mut handles = vec![stop];
            let mut events = Vec::new();
            if wait_low {
                handles.push(notif.low);
                events.push(PressureEvent::Low);
            }
            if wait_high {
                handles.push(notif.high);
                events.push(PressureEvent::High);
            }
            // SAFETY: 全て有効なハンドルで、呼び出しの間生きる（通知は本関数が所有、停止は親が所有）。
            let r = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
            if r == WAIT_OBJECT_0 {
                return;
            }
            let idx = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            match idx.checked_sub(1).and_then(|i| events.get(i)) {
                Some(e) => fire(id, Signal::Event(*e)),
                None => {
                    // WAIT_FAILED・WAIT_ABANDONED など規定外は実行中の失敗にする。
                    fire(id, Signal::Failed);
                    return;
                }
            }
        }
    }

    /// 実行中の購読。drop で停止する。
    struct WinGuard {
        id: u64,
        stop: HANDLE,
        thread: Option<JoinHandle<()>>,
    }

    impl SubscriptionGuard for WinGuard {}

    // SAFETY: `HANDLE` は停止イベントで、所有権は guard が持つ。スレッド間で触るのは SetEvent / CloseHandle
    // のみ（いずれもスレッドセーフ）で、CloseHandle はワーカーの終了を join で待ってから行う。
    // trait が `SubscriptionGuard: Send` を要求するため明示する。
    #[allow(unsafe_code)]
    unsafe impl Send for WinGuard {}

    impl Drop for WinGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから、スレッドを止めて終了を待つ。
            unregister(self.id);
            // SAFETY: `stop` は本構造体が所有する有効なイベント。
            let _ = unsafe { SetEvent(self.stop) };
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            // SAFETY: スレッド終了後なので他に使う者はいない。二重に閉じない。
            let _ = unsafe { CloseHandle(self.stop) };
        }
    }

    pub(super) struct WindowsMemoryPressure;

    impl MemoryPressure for WindowsMemoryPressure {
        fn support(&self) -> EventSupport {
            // 順序は ALL_EVENTS: Low（添字 5）・High（添字 6）だけ。今の状態は読める。
            let mut events = [false; 11];
            events[PressureEvent::Low.index()] = true;
            events[PressureEvent::High.index()] = true;
            EventSupport {
                events,
                current: true,
            }
        }

        #[allow(unsafe_code)]
        fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
            let notif = Notifications::create().ok_or(START_FAILED)?;
            // 規定外の状態（両方 signaled）や問い直しの失敗は、待機を始める前に開始失敗にする。
            notif.level().map_err(|_| START_FAILED)?;
            // SAFETY: 引数は有効な既定値（manual-reset・非シグナル）。
            let stop =
                unsafe { CreateEventW(None, true, false, None) }.map_err(|_| START_FAILED)?;
            let id = register(on_signal);
            let (tx, rx) = sync_channel::<()>(1);
            let stop_raw = stop.0 as usize;
            let thread = std::thread::Builder::new()
                .name("fandhe-memory-pressure".into())
                .spawn(move || {
                    let _ = tx.send(());
                    run(id, notif, stop_raw)
                });
            match thread {
                Ok(t) => {
                    // 待つのはスレッドの起動合図だけ（ローカルの処理）。
                    let _ = rx.recv();
                    Ok(Box::new(WinGuard {
                        id,
                        stop,
                        thread: Some(t),
                    }))
                }
                Err(_) => {
                    // `notif` はクロージャごと drop 済みで閉じられている。残りを片付ける。
                    unregister(id);
                    // SAFETY: 本関数が作った有効なイベントで、スレッドは起動していない。
                    let _ = unsafe { CloseHandle(stop) };
                    Err(START_FAILED)
                }
            }
        }

        fn read_current(&self) -> Result<Option<MemoryLevel>, DeviceError> {
            // 呼ぶたびに作って問い、閉じる（アプリ内に覚えない）。
            let notif = Notifications::create().ok_or(READ_FAILED)?;
            notif.level().map(Some)
        }
    }
}

/// この OS の実装。Windows 以外（ホストのテスト）は非対応を返す。
pub(crate) fn implementation() -> Arc<dyn MemoryPressure> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(imp::WindowsMemoryPressure)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Arc::new(UnsupportedMemoryPressure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn host_implementation_never_succeeds() {
        let imp = implementation();
        assert!(imp.start(Arc::new(|_| {})).is_err());
        assert!(imp.read_current().is_err());
    }
}
