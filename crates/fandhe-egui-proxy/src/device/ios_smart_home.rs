//! iOS 向けスマートホーム実装のスタブ（Issue #1079、親 #147）。`device/smart_home.rs` の iOS 経路の
//! dispatch 先で、後続の子 #1080 が実体を入れるまで常に `Unsupported` を返す
//! （ダミーの成功値へは倒さない）。
//!
//! 後続が埋める範囲: `ios/fandhe_ios_smart_home.m`（main queue で `HMHomeManager` を 1 つ生成して保持、
//! delegate 待ち、semaphore での期限、期限切れ時に `dispatch_sync(main)` で待ち登録を外す、
//! `@autoreleasepool`）、`build.rs` での HomeKit のリンク、`NSHomeKitUsageDescription`、entitlement。
//! 後続は本ファイル内の `#[cfg(target_os = "ios")]` の項目と OS 固有の付帯物だけを触る。
//! dispatch・画面・fallback 行・`device.rs` の `pub mod` 列は触らない。

use std::sync::Arc;

use fandhe_device::smart_home::{SmartHome, UnsupportedSmartHome};

/// スマートホームの実装を返す（実体は #1080）。
pub(crate) fn implementation() -> Arc<dyn SmartHome> {
    Arc::new(UnsupportedSmartHome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::smart_home::SmartHomeFailure;
    use std::time::Duration;

    #[test]
    fn stub_never_succeeds() {
        assert_eq!(
            implementation()
                .list_accessories(Duration::from_millis(1))
                .map(|_| ()),
            Err(SmartHomeFailure::Unsupported)
        );
    }
}
