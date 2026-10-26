//! iOS 向けメモリ逼迫の通知実装（Issue #912）。`ios/fandhe_ios_memory_pressure.m`
//! （メモリ警告の main キュー observer と `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE`。`build.rs` が iOS 分岐で
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこの呼び出しに
//! 閉じ `// SAFETY:` で契約を明記する。`device/memory_pressure.rs` の ios 経路から呼ばれ、
//! 契約は `fandhe_device::memory_pressure`。
//!
//! # 方式
//!
//! OS 資源（observer トークンと source）は `id` で `.m` が所有し、Rust へは生ポインタを渡さない。
//! 合図は `.m` が `fandhe_rust_memory_pressure_fire`（kind 0 = 段階、kind 1 = メモリ警告。符号化は
//! `fandhe_device::memory_pressure::event_from_apple`）経由で `id` の登録表を引いて届ける。
//! 再描画要求は dispatch 側の `wake` が行う。停止（`IosGuard` の drop）は「登録解除 → `.m` の解除」の順。
//!
//! # OS を正にする範囲
//!
//! メモリ警告と段階の変化 3 種を知らせる。起動時の段階は合図にしない。今の段階を読む公開 API は無く、
//! `support().current` は false、`read_current` は `Ok(None)`。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗で登録表・observer・source を残さない。drop 後に届いた合図は登録表で捨てる
//! - 守らない範囲: 画面を開く前・背景で停止中の合図、合図の到着順と OS の実状態の時間差

use fandhe_device::memory_pressure::EventSupport;

/// 対応する合図。順序は ALL_EVENTS: 添字 0 = メモリ警告、1〜3 = 平常・警告・危機的。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
const SUPPORT: EventSupport = {
    let mut events = [false; 11];
    events[0] = true;
    events[1] = true;
    events[2] = true;
    events[3] = true;
    EventSupport {
        events,
        current: false,
    }
};

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::Arc;

    use fandhe_device::memory_pressure::{
        EventSupport, MemoryLevel, MemoryPressure, OnSignal, SubscriptionGuard,
    };
    use fandhe_device::DeviceError;

    use super::super::memory_pressure::{register, unregister};
    use super::SUPPORT;

    mod ffi {
        // SAFETY: `ios/fandhe_ios_memory_pressure.m`（`build.rs` が iOS 分岐で feature `device` 有効時に
        // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
        #[allow(unsafe_code)]
        extern "C" {
            fn fandhe_ios_memory_pressure_start(id: u64) -> i32;
            fn fandhe_ios_memory_pressure_stop(id: u64);
        }

        /// 開始に成功したか。
        #[allow(unsafe_code)]
        pub(super) fn start(id: u64) -> bool {
            // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。ポインタは渡さない。
            unsafe { fandhe_ios_memory_pressure_start(id) == 0 }
        }

        /// 停止して解放する。未知の `id` は何もしない。
        #[allow(unsafe_code)]
        pub(super) fn stop(id: u64) {
            // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。未知の id は no-op。
            unsafe { fandhe_ios_memory_pressure_stop(id) }
        }
    }

    /// 実行中の購読。drop で停止する。
    struct IosGuard {
        id: u64,
    }

    impl SubscriptionGuard for IosGuard {}

    impl Drop for IosGuard {
        fn drop(&mut self) {
            // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから OS の資源を外す。
            unregister(self.id);
            ffi::stop(self.id);
        }
    }

    struct IosMemoryPressure;

    impl MemoryPressure for IosMemoryPressure {
        fn support(&self) -> EventSupport {
            SUPPORT
        }

        fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
            let id = register(on_signal);
            if !ffi::start(id) {
                unregister(id);
                return Err(DeviceError::Internal("memory_pressure start"));
            }
            Ok(Box::new(IosGuard { id }))
        }

        fn read_current(&self) -> Result<Option<MemoryLevel>, DeviceError> {
            Ok(None)
        }
    }

    /// iOS の `MemoryPressure` 実装を返す（`device/memory_pressure.rs` の dispatch から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn MemoryPressure> {
        Arc::new(IosMemoryPressure)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::memory_pressure::{event_from_apple, PressureEvent};

    #[test]
    fn support_declares_memory_warning_and_levels_only() {
        let s = std::hint::black_box(SUPPORT);
        assert!(!s.current);
        assert!(s.supports(PressureEvent::MemoryWarning));
        assert!(s.supports(PressureEvent::Normal));
        assert!(s.supports(PressureEvent::Warning));
        assert!(s.supports(PressureEvent::Critical));
        assert!(!s.supports(PressureEvent::Trim(0)));
        assert!(!s.supports(PressureEvent::Low));
        assert!(!s.supports(PressureEvent::High));
    }

    // `.m` が送る kind / flags との対応を固定する。通知と実機の経路はホストでは試せず #884 で確かめる。
    #[test]
    fn apple_encoding_matches_objc_side() {
        assert_eq!(event_from_apple(1, 0), Some(PressureEvent::MemoryWarning));
        assert_eq!(event_from_apple(0, 0x1), Some(PressureEvent::Normal));
        assert_eq!(event_from_apple(0, 0x2), Some(PressureEvent::Warning));
        assert_eq!(event_from_apple(0, 0x4), Some(PressureEvent::Critical));
    }
}
