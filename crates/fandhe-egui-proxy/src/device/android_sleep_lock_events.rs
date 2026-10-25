//! Android 向けスリープ・ロックの通知の骨格（Issue #810 が置く `Unsupported` のスタブ。
//! 実体は #811）。`device/sleep_lock_events.rs` の Android 経路から呼ばれる。
//!
//! # #811 への約束
//!
//! - 触ってよいのは本ファイル・Java ヘルパー・JNI・`MainActivity` の最小追記だけ。dispatch
//!   （`platform_sleep_lock_events`）・fallback 行・`device.rs` の `pub mod` 列・共通コアの画面・
//!   `registry.rs` は触らない（#810 で全対象 OS の最終形にしてある）
//! - OS コールバックは `super::sleep_lock_events` の `register` / `unregister` / `fire` を使い、
//!   生ポインタではなく `id` を持つ。JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - 知らせる種類は画面の消灯・点灯と解除（`ACTION_SCREEN_OFF` / `ON` / `USER_PRESENT`）、
//!   読める状態は `isInteractive` / `isKeyguardLocked`。スリープとロックした瞬間は取れない
//! - Activity 再生成・`onPause` の扱い（`mark_left` か停止）を決める。経過時間・時刻・タイマーを使わない

use std::sync::Arc;

use fandhe_device::sleep_lock_events::{SleepLockEvents, UnsupportedSleepLockEvents};

/// Android の `SleepLockEvents` 実装を返す（#811 までは非対応を返すスタブ）。
pub(crate) fn implementation() -> Arc<dyn SleepLockEvents> {
    Arc::new(UnsupportedSleepLockEvents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_unsupported() {
        let imp = implementation();
        assert!(matches!(
            imp.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("sleep_lock_events"))
        ));
        assert!(matches!(
            imp.read_current(),
            Err(DeviceError::Unsupported("sleep_lock_events"))
        ));
    }
}
