//! iOS 向けアプリの前面・背面の通知実装（Issue #894）。`ios/fandhe_ios_app_lifecycle.m`
//! （`UIScene` の通知。`build.rs` が iOS 分岐で feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 3 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/app_lifecycle.rs` の ios 経路から呼ばれ、契約は `fandhe_device::app_lifecycle`。
//!
//! # 方式
//!
//! OS 資源は `id` で `.m` が所有し、Rust へは生ポインタを渡さない。通知は `.m` が
//! `fandhe_rust_app_lifecycle_fire`（`device/app_lifecycle.rs`。届けた直後に `request_repaint` で
//! 次フレームを起こす）経由で `id` の登録表を引いて出来事にする。
//! 停止（`IosGuard` の drop）は「登録解除 → `.m` の observer 解除」の順。
//!
//! # OS を正にする範囲
//!
//! application ロールの scene の 4 通知（didActivate / willDeactivate / didEnterBackground /
//! willEnterForeground）を知らせる。凍結・bfcache 復帰は取れず、`support()` で false にする。
//! 今の状態は `activationState` を main thread で読む。main thread 以外・規定外の値は `Err` で、
//! scene 未接続・Unattached は「不明」。前面へ畳まない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗で登録表と observer を残さない。drop が戻った後に届いた通知は登録表で捨てる
//! - 守らない範囲: suspended への通知（無い）、通知の到着順と OS の実状態の時間差、外部画面の scene

use fandhe_device::app_lifecycle::EventSupport;
#[cfg(target_os = "ios")]
use fandhe_device::app_lifecycle::{
    current_from_apple_bits, AppLifecycle, CurrentState, OnSignal, SubscriptionGuard,
};
#[cfg(target_os = "ios")]
use fandhe_device::DeviceError;
#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use super::app_lifecycle::{register, unregister};

#[cfg(target_os = "ios")]
mod ffi {
    // SAFETY: `ios/fandhe_ios_app_lifecycle.m`（`build.rs` が iOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_app_lifecycle_start(id: u64) -> i32;
        fn fandhe_ios_app_lifecycle_stop(id: u64);
        fn fandhe_ios_app_lifecycle_read() -> i32;
    }

    /// 開始に成功したか。
    #[allow(unsafe_code)]
    pub(super) fn start(id: u64) -> bool {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。ポインタは渡さない。
        unsafe { fandhe_ios_app_lifecycle_start(id) == 0 }
    }

    /// 停止して解放する。未知の `id` は何もしない。
    #[allow(unsafe_code)]
    pub(super) fn stop(id: u64) {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。未知の id は no-op。
        unsafe { fandhe_ios_app_lifecycle_stop(id) }
    }

    /// 今の状態の符号化（負は失敗）。
    #[allow(unsafe_code)]
    pub(super) fn read() -> i32 {
        // SAFETY: 引数なしで、`.m` は main thread でなければ失敗コードを返す。
        unsafe { fandhe_ios_app_lifecycle_read() }
    }
}

/// 宣言する対応範囲。順序は ALL_EVENTS: 前面・背面の 4 種は取れ、凍結 3 種は取れない。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
const SUPPORT: EventSupport = EventSupport {
    events: [true, true, true, true, false, false, false],
    active: true,
    visible: true,
};

/// 実行中の購読。drop で停止する。
#[cfg(target_os = "ios")]
struct IosGuard {
    id: u64,
}

#[cfg(target_os = "ios")]
impl SubscriptionGuard for IosGuard {}

#[cfg(target_os = "ios")]
impl Drop for IosGuard {
    fn drop(&mut self) {
        // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから OS の observer を外す。
        unregister(self.id);
        ffi::stop(self.id);
    }
}

#[cfg(target_os = "ios")]
struct IosAppLifecycle;

#[cfg(target_os = "ios")]
impl AppLifecycle for IosAppLifecycle {
    fn support(&self) -> EventSupport {
        SUPPORT
    }

    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        let id = register(on_signal);
        if !ffi::start(id) {
            unregister(id);
            return Err(DeviceError::Internal("app_lifecycle start"));
        }
        Ok(Box::new(IosGuard { id }))
    }

    fn read_current(&self) -> Result<CurrentState, DeviceError> {
        current_from_apple_bits(ffi::read())
    }
}

/// iOS の `AppLifecycle` 実装を返す（`device/app_lifecycle.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn AppLifecycle> {
    Arc::new(IosAppLifecycle)
}

// 通知や実機の経路はホストでは試せない。シミュレータ・実機の目視は #883 で確かめる。
#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::app_lifecycle::current_from_apple_bits;

    #[test]
    fn support_declares_frozen_as_unavailable() {
        let s = std::hint::black_box(SUPPORT);
        assert!(s.active && s.visible);
        assert!(s.events[..4].iter().all(|e| *e));
        assert!(s.events[4..].iter().all(|e| !*e));
    }

    // `.m` が返す符号（ForegroundActive = 5 / Inactive = 4 / Background = 0 / 不明 = 10 / 失敗 = -1, -2）を
    // Rust 側の解釈と突き合わせ、失敗が前面へ畳まれず Err になることを固定する。
    #[test]
    fn apple_bits_map_without_folding_failures() {
        for ok in [5, 4, 0, 10] {
            assert!(current_from_apple_bits(ok).is_ok(), "{ok}");
        }
        for ng in [-1, -2] {
            assert!(current_from_apple_bits(ng).is_err(), "{ng}");
        }
    }
}
