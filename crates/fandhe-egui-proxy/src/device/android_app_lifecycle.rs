//! Android 向けアプリの前面・背面の通知の骨格（Issue #893 の (a)、実体は b-android）。
//! `device/app_lifecycle.rs::platform_app_lifecycle` の android 経路から呼ばれ、いまは `Unsupported` を返す
//! だけのスタブ。ダミーの成功実装へは倒さない。契約は `fandhe_device::app_lifecycle`。
//!
//! # b-android への約束
//!
//! - 触ってよいのはこのファイル、Java（`MainActivity` と補助クラス）、対応表の Android セルだけ。
//!   dispatch・共通コア画面・`fandhe-device` は触らない
//! - `MainActivity` の `onResume` / `onPause` / `onStart` / `onStop` に 1 行ずつ足し、`id` 付きの
//!   `nativeOnAppLifecycle(id, code)` へ流す（code は `event_from_apple_code` の 0〜3 と同じ意味）。
//!   `isChangingConfigurations()` が真の間は出来事を出さない（再生成は前面・背面の変化ではない）
//! - 今の状態は `ActivityManager.getMyMemoryState` の `importance` から作り、符号化は
//!   `fandhe_device::app_lifecycle::current_from_apple_bits` に揃える。読めなければ負を返して失敗にする
//! - JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - `onDestroy` で購読を解除し、停止は失敗にしない。再生成後の古い `id` の通知は登録表が捨てる
//!   （画面側の `mark_left` で「画面を離れたため止めました」にする）
//! - 凍結・bfcache 復帰は取れない（`support()` で false）

use std::sync::Arc;

use fandhe_device::app_lifecycle::{AppLifecycle, UnsupportedAppLifecycle};

/// Android の `AppLifecycle` 実装を返す（実体が入るまでは非対応）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn implementation() -> Arc<dyn AppLifecycle> {
    Arc::new(UnsupportedAppLifecycle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::app_lifecycle::EventSupport;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_never_succeeds() {
        let imp = implementation();
        assert_eq!(imp.support(), EventSupport::NONE);
        assert!(matches!(
            imp.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("app_lifecycle"))
        ));
        assert!(matches!(
            imp.read_current(),
            Err(DeviceError::Unsupported("app_lifecycle"))
        ));
    }
}
