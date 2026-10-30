//! iOS 向けヘルスデータ（歩数）実装のスタブ（Issue #1089、親 #149）。`device/health_data.rs` の iOS
//! 経路の dispatch 先で、後続の子 #1090 が実体を入れるまで常に `Unsupported` を返す
//! （ダミーの成功値へは倒さない）。
//!
//! 後続が埋める範囲: `ios/fandhe_ios_health_data.m`。FFI は
//! `fandhe_ios_health_data_read(uint64_t timeout_ms, double *out_steps) -> int32_t` で、戻りコードは
//! `fandhe_device::health_data::decode_code` の表に従う。`@autoreleasepool`、static の `HKHealthStore`、
//! 許可待ちと集計待ちで期限を共有（`dispatch_time`）、期限切れで `stopQuery:`、許可待ちの旗による
//! 多重起動の拒否、`HKErrorDomain` を確かめてからコードを見る。加えて `build.rs` の iOS 分岐での
//! HealthKit のリンク、`NSHealthShareUsageDescription`、`FandheIosApp.entitlements` への
//! `com.apple.developer.healthkit`。
//! 後続は本ファイルの `#[cfg(target_os = "ios")]` の項目と OS 固有の付帯物だけを触る。
//! dispatch・画面・fallback 行・`device.rs` の `pub mod` 列は触らない。

use std::sync::Arc;

use fandhe_device::health_data::{HealthData, UnsupportedHealthData};

/// ヘルスデータの実装を返す（実体は #1090）。
pub(crate) fn implementation() -> Arc<dyn HealthData> {
    Arc::new(UnsupportedHealthData)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::health_data::HealthDataFailure;
    use std::time::Duration;

    #[test]
    fn stub_never_succeeds() {
        assert_eq!(
            implementation().read_today_steps(Duration::from_millis(1)),
            Err(HealthDataFailure::Unsupported)
        );
    }
}
