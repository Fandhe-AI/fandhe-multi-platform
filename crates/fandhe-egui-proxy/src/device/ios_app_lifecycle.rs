//! iOS 向けアプリの前面・背面の通知の骨格（Issue #893 の (a)、実体は b-ios）。
//! `device/app_lifecycle.rs::platform_app_lifecycle` の ios 経路から呼ばれ、いまは `Unsupported` を返す
//! だけのスタブ。ダミーの成功実装へは倒さない。契約は `fandhe_device::app_lifecycle`。
//!
//! # b-ios への約束
//!
//! - 触ってよいのはこのファイル、`ios/` の `.m`、`build.rs` の iOS 分岐、対応表の iOS セルだけ。
//!   dispatch・共通コア画面・`fandhe-device` は触らない（`fandhe_rust_app_lifecycle_fire` の受け口と
//!   iOS の再描画要求は dispatch 側に置いてある）
//! - 本アプリは scene 対応のため `UIScene` の 4 通知（didActivate = 0 / willDeactivate = 1 /
//!   didEnterBackground = 2 / willEnterForeground = 3）を使う。`UIApplication` の通知は使わない
//! - 見るのは application ロールの scene だけ（外部画面の scene を数えない）。`fandhe_app_scene()` を使う
//! - 通知の登録トークンは `id` をキーに握り続ける（契約表 1）。停止で全部外す
//! - 今の状態の `activationState` は main thread で読み、
//!   `fandhe_device::app_lifecycle::current_from_apple_bits` の符号化で返す。scene が未接続なら不明
//! - `.m` は各関数を `@autoreleasepool` で囲み、`dispatch_sync` をロックの内側で使わない
//! - 凍結・bfcache 復帰は取れない（`support()` で false）

use std::sync::Arc;

use fandhe_device::app_lifecycle::{AppLifecycle, UnsupportedAppLifecycle};

/// iOS の `AppLifecycle` 実装を返す（実体が入るまでは非対応）。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
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
