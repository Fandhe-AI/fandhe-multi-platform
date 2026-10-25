//! macOS 向けスリープ・ロックの通知実装（Issue #810）。`macos/fandhe_macos_sleep_lock_events.m`
//! （NSWorkspace の通知センター。`build.rs` が macOS 分岐で feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/sleep_lock_events.rs` の macOS 経路から UI スレッドで呼ばれ、契約は
//! `fandhe_device::sleep_lock_events`。
//!
//! # 方式
//!
//! OS 資源は `id` で `.m` が所有し、Rust へは生ポインタを渡さない。通知は `.m` が
//! `fandhe_rust_sleep_lock_events_fire`（`device/sleep_lock_events.rs`）経由で `id` の登録表を引いて
//! 出来事にする。停止（[`MacGuard`] の drop）は「登録解除 → `.m` の observer 解除」の順。
//!
//! # OS を正にする範囲
//!
//! スリープ・復帰・画面の消灯・点灯の 4 種を知らせる。画面ロックは公開 API が無く取れない
//! （`support()` で false）。今の状態も読めないため `read_current` は `lock: None, screen: None`
//! （読めないことを「無し」で返し、失敗にも成功値の捏造にもしない）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗で登録表と observer を残さない。drop が戻った後に届いた通知は登録表で捨てる
//! - 守らない範囲: `willSleep` の通知から実際のスリープまでの時間差、ロックの通知

use std::sync::Arc;

use fandhe_device::sleep_lock_events::{
    CurrentState, EventSupport, OnSignal, SleepLockEvents, SubscriptionGuard,
};
use fandhe_device::DeviceError;

use super::sleep_lock_events::{register, unregister};

mod ffi {
    // SAFETY: `macos/fandhe_macos_sleep_lock_events.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_sleep_lock_events_start(id: u64) -> i32;
        fn fandhe_macos_sleep_lock_events_stop(id: u64);
    }

    /// 開始に成功したか。
    #[allow(unsafe_code)]
    pub(super) fn start(id: u64) -> bool {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。ポインタは渡さない。
        unsafe { fandhe_macos_sleep_lock_events_start(id) == 0 }
    }

    /// 停止して解放する。未知の `id` は何もしない。
    #[allow(unsafe_code)]
    pub(super) fn stop(id: u64) {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。未知の id は no-op。
        unsafe { fandhe_macos_sleep_lock_events_stop(id) }
    }
}

/// 実行中の購読。drop で停止する。
struct MacGuard {
    id: u64,
}

impl SubscriptionGuard for MacGuard {}

impl Drop for MacGuard {
    fn drop(&mut self) {
        // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから OS の observer を外す。
        unregister(self.id);
        ffi::stop(self.id);
    }
}

struct MacSleepLockEvents;

impl SleepLockEvents for MacSleepLockEvents {
    fn support(&self) -> EventSupport {
        // 順序は ALL_EVENTS: スリープ・復帰・画面の消灯・点灯は取れる、ロック・解除は取れない。
        EventSupport {
            events: [true, true, true, true, false, false],
            lock: false,
            screen: false,
        }
    }

    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        let id = register(on_signal);
        if !ffi::start(id) {
            unregister(id);
            return Err(DeviceError::Internal("sleep_lock_events start"));
        }
        Ok(Box::new(MacGuard { id }))
    }

    fn read_current(&self) -> Result<CurrentState, DeviceError> {
        Ok(CurrentState {
            lock: None,
            screen: None,
        })
    }
}

/// macOS の `SleepLockEvents` 実装を返す（`device/sleep_lock_events.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn SleepLockEvents> {
    Arc::new(MacSleepLockEvents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_stop_start_succeeds_and_leaves_nothing() {
        let imp = implementation();
        let g1 = imp.start(Arc::new(|_| {})).expect("開始できる");
        drop(g1);
        let g2 = imp.start(Arc::new(|_| {})).expect("再開始できる");
        drop(g2);
    }

    #[test]
    fn double_start_with_same_id_fails_without_side_effects() {
        let id = register(Arc::new(|_| {}));
        assert!(ffi::start(id));
        assert!(!ffi::start(id), "同じ id の二重登録は失敗");
        ffi::stop(id);
        // stop の後は同じ id で再登録できる（辞書が空になっている）。
        assert!(ffi::start(id));
        ffi::stop(id);
        unregister(id);
    }

    #[test]
    fn support_declares_lock_as_unavailable() {
        let s = implementation().support();
        assert!(!s.lock && !s.screen);
        assert!(s.events[0] && s.events[3]);
        assert!(!s.events[4] && !s.events[5]);
        let c = implementation().read_current().unwrap();
        assert!(c.lock.is_none() && c.screen.is_none());
    }
}
