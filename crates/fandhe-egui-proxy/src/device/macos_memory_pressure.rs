//! macOS 向けメモリ逼迫の通知実装（Issue #911）。`macos/fandhe_macos_memory_pressure.m`
//! （libdispatch の `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE`。`build.rs` が macOS 分岐で feature `device`
//! 有効時にコンパイルする）を `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ
//! `// SAFETY:` で契約を明記する。`device/memory_pressure.rs` の macOS 経路から UI スレッドで呼ばれ、
//! 契約は `fandhe_device::memory_pressure`。
//!
//! # 方式
//!
//! OS 資源は `id` で `.m` が所有し、Rust へは生ポインタを渡さない。合図は `.m` が
//! `fandhe_rust_memory_pressure_fire`（`device/memory_pressure.rs`）経由で `id` の登録表を引いて
//! 段階（平常・警告・危機的）にする。停止（[`MacGuard`] の drop）は「登録解除 → `.m` の source 解除」の順。
//!
//! # OS を正にする範囲
//!
//! 段階の変化 3 種を知らせる。起動時の段階は合図にしない。今の段階を読む公開 API は無く、非公開の
//! sysctl は使わないため `support().current` は false、`read_current` は `Ok(None)`。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗で登録表と source を残さない。drop が戻った後に届いた合図は登録表で捨てる
//! - 守らない範囲: 画面を開く前の段階、合図の到着順と OS の実状態の時間差、システム全体の値であること

use std::sync::Arc;

use fandhe_device::memory_pressure::{
    EventSupport, MemoryLevel, MemoryPressure, OnSignal, SubscriptionGuard,
};
use fandhe_device::DeviceError;

use super::memory_pressure::{register, unregister};

mod ffi {
    // SAFETY: `macos/fandhe_macos_memory_pressure.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_memory_pressure_start(id: u64) -> i32;
        fn fandhe_macos_memory_pressure_stop(id: u64);
    }

    /// 開始に成功したか。
    #[allow(unsafe_code)]
    pub(super) fn start(id: u64) -> bool {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。ポインタは渡さない。
        unsafe { fandhe_macos_memory_pressure_start(id) == 0 }
    }

    /// 停止して解放する。未知の `id` は何もしない。
    #[allow(unsafe_code)]
    pub(super) fn stop(id: u64) {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。未知の id は no-op。
        unsafe { fandhe_macos_memory_pressure_stop(id) }
    }
}

/// 実行中の購読。drop で停止する。
struct MacGuard {
    id: u64,
}

impl SubscriptionGuard for MacGuard {}

impl Drop for MacGuard {
    fn drop(&mut self) {
        // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから OS の source を外す。
        unregister(self.id);
        ffi::stop(self.id);
    }
}

struct MacMemoryPressure;

impl MemoryPressure for MacMemoryPressure {
    fn support(&self) -> EventSupport {
        // 順序は ALL_EVENTS: 平常・警告・危機的（添字 1〜3）だけ。今の段階は読めない。
        let mut events = [false; 11];
        events[1..=3].fill(true);
        EventSupport {
            events,
            current: false,
        }
    }

    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        let id = register(on_signal);
        if !ffi::start(id) {
            unregister(id);
            return Err(DeviceError::Internal("memory_pressure start"));
        }
        Ok(Box::new(MacGuard { id }))
    }

    fn read_current(&self) -> Result<Option<MemoryLevel>, DeviceError> {
        Ok(None)
    }
}

/// macOS の `MemoryPressure` 実装を返す（`device/memory_pressure.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn MemoryPressure> {
    Arc::new(MacMemoryPressure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::memory_pressure::PressureEvent;

    #[test]
    fn stop_of_unknown_id_is_a_noop() {
        ffi::stop(u64::MAX);
    }

    #[test]
    fn support_declares_only_levels_and_no_current() {
        let s = implementation().support();
        assert!(!s.current);
        assert!(s.supports(PressureEvent::Normal));
        assert!(s.supports(PressureEvent::Warning));
        assert!(s.supports(PressureEvent::Critical));
        assert!(!s.supports(PressureEvent::MemoryWarning));
        assert!(!s.supports(PressureEvent::Trim(0)));
        assert!(!s.supports(PressureEvent::Low));
    }

    #[test]
    fn current_is_not_available_not_normal() {
        assert!(matches!(implementation().read_current(), Ok(None)));
    }

    // 合図が届く経路（段階の変化）は Mac 目視（#884）で確かめる。ここでは開始・停止が資源を残さないことだけを見る。
    #[test]
    fn start_then_drop_unregisters() {
        let imp = implementation();
        let guard = imp
            .start(Arc::new(|_| {}))
            .expect("dispatch source は作れる");
        drop(guard);
        let guard = imp.start(Arc::new(|_| {})).expect("停止後に再開できる");
        drop(guard);
    }
}
