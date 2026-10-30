//! Android 向けヘルスデータ（今日の歩数）実装の骨格（Issue #1099、親 #149）。
//! `device/health_data.rs` の Android 経路から呼ばれる。実体（Java 側 `HealthDataHelper` との
//! JNI 橋渡し）は #1100 がこのファイルと Java 側だけを書いて埋める。
//!
//! #1100 への約束: dispatch・fallback・`device.rs` の `pub mod` 列・共通コア画面は触らない。
//! 写像（API 33 以下は非対応、許可なしは `NoVisibleData` など）は
//! `fandhe_device::health_data` の doc「Android の写像」に従う。それまでは非対応を返し、
//! 成功へは倒さない。

use std::sync::Arc;

use fandhe_device::health_data::{HealthData, UnsupportedHealthData};

/// Android の `HealthData` 実装を返す（`device/health_data.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn HealthData> {
    Arc::new(UnsupportedHealthData)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::health_data::{HealthDataFailure, READ_TIMEOUT};

    #[test]
    fn stub_is_not_a_fake_success() {
        assert_eq!(
            implementation().read_today_steps(READ_TIMEOUT),
            Err(HealthDataFailure::Unsupported)
        );
    }
}
