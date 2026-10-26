//! macOS 向けアプリの前面・背面の通知実装（Issue #893）。`macos/fandhe_macos_app_lifecycle.m`
//! （`NSApplication` の通知。`build.rs` が macOS 分岐で feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 3 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/app_lifecycle.rs` の macOS 経路から UI スレッドで呼ばれ、契約は
//! `fandhe_device::app_lifecycle`。
//!
//! # 方式
//!
//! OS 資源は `id` で `.m` が所有し、Rust へは生ポインタを渡さない。通知は `.m` が
//! `fandhe_rust_app_lifecycle_fire`（`device/app_lifecycle.rs`）経由で `id` の登録表を引いて出来事にする。
//! 停止（[`MacGuard`] の drop）は「登録解除 → `.m` の observer 解除」の順。
//!
//! # OS を正にする範囲
//!
//! 操作の対象になった・外れた、見えるようになった・見えなくなった（`occlusionState` を通知のたびに
//! 問い直す）の 4 種を知らせる。凍結（App Nap）の通知は無く、`support()` で false にする。今の状態は
//! `isActive` と `occlusionState` を UI スレッドで読む。UI スレッド以外・`NSApp` 不在は `Err` で、
//! 前面・見えているへ畳まない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗で登録表と observer を残さない。drop が戻った後に届いた通知は登録表で捨てる
//! - 守らない範囲: 通知の到着順と OS の実状態の時間差、複数窓の個別の可視状態、App Nap

use std::sync::Arc;

use fandhe_device::app_lifecycle::{
    current_from_apple_bits, AppLifecycle, CurrentState, EventSupport, OnSignal, SubscriptionGuard,
};
use fandhe_device::DeviceError;

use super::app_lifecycle::{register, unregister};

mod ffi {
    // SAFETY: `macos/fandhe_macos_app_lifecycle.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_app_lifecycle_start(id: u64) -> i32;
        fn fandhe_macos_app_lifecycle_stop(id: u64);
        fn fandhe_macos_app_lifecycle_read() -> i32;
    }

    /// 開始に成功したか。
    #[allow(unsafe_code)]
    pub(super) fn start(id: u64) -> bool {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。ポインタは渡さない。
        unsafe { fandhe_macos_app_lifecycle_start(id) == 0 }
    }

    /// 停止して解放する。未知の `id` は何もしない。
    #[allow(unsafe_code)]
    pub(super) fn stop(id: u64) {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。未知の id は no-op。
        unsafe { fandhe_macos_app_lifecycle_stop(id) }
    }

    /// 今の状態の符号化（負は失敗）。
    #[allow(unsafe_code)]
    pub(super) fn read() -> i32 {
        // SAFETY: 引数なしで、`.m` は main thread でなければ失敗コードを返す。
        unsafe { fandhe_macos_app_lifecycle_read() }
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

struct MacAppLifecycle;

impl AppLifecycle for MacAppLifecycle {
    fn support(&self) -> EventSupport {
        // 順序は ALL_EVENTS: 操作の対象・見えているの 4 種は取れる。凍結 3 種は取れない。
        EventSupport {
            events: [true, true, true, true, false, false, false],
            active: true,
            visible: true,
        }
    }

    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        let id = register(on_signal);
        if !ffi::start(id) {
            unregister(id);
            return Err(DeviceError::Internal("app_lifecycle start"));
        }
        Ok(Box::new(MacGuard { id }))
    }

    fn read_current(&self) -> Result<CurrentState, DeviceError> {
        current_from_apple_bits(ffi::read())
    }
}

/// macOS の `AppLifecycle` 実装を返す（`device/app_lifecycle.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn AppLifecycle> {
    Arc::new(MacAppLifecycle)
}

#[cfg(test)]
mod tests {
    use super::*;

    // テストプロセスには NSApplication が無い（`NSApp` が nil）ため、開始は失敗し、何も残さない。
    // 実際に通知を受ける経路は Mac 目視（#883）で確かめる。
    #[test]
    fn start_without_application_fails_and_leaves_nothing() {
        let imp = implementation();
        assert!(matches!(
            imp.start(Arc::new(|_| {})),
            Err(DeviceError::Internal("app_lifecycle start"))
        ));
        // 失敗後も同じ操作を繰り返せる（辞書・登録表に残っていない）。
        assert!(imp.start(Arc::new(|_| {})).is_err());
    }

    #[test]
    fn stop_of_unknown_id_is_a_noop() {
        ffi::stop(u64::MAX);
    }

    #[test]
    fn support_declares_frozen_as_unavailable() {
        let s = implementation().support();
        assert!(s.active && s.visible);
        assert!(s.events[..4].iter().all(|e| *e));
        assert!(s.events[4..].iter().all(|e| !*e));
    }

    #[test]
    fn read_off_main_thread_is_an_error_not_a_state() {
        // テストスレッドは main thread ではないため、前面・見えているへ畳まず失敗にする。
        assert!(implementation().read_current().is_err());
    }
}
