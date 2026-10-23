//! Windows 向け気圧・照度・近接センサー実装のスタブ（Issue #609、親 #123）。`device/environment_sensors.rs` の
//! Windows 経路の dispatch 先で、後続の子 #612 が実体を入れるまで全種類で `UnsupportedOnOs` を返す
//! （ダミーの成功値へは倒さない）。
//!
//! 後続が埋める範囲: 気圧・照度は同期 pull、近接は作業スレッドで 1 回だけ列挙して世代を付け、
//! 停止後の結果を捨てる。`E_ACCESSDENIED` は `PermissionDenied`。`ReportInterval` の設定に失敗したら
//! 0 へ戻して保持を捨てる。
//! 後続は本ファイル内の `#[cfg(target_os = "windows")]` の項目だけを触る。
//! dispatch・画面・fallback 行・`device.rs` の `pub mod` 列は触らない。

use std::sync::Arc;

use fandhe_device::environment_sensors::{EnvironmentSensors, UnsupportedEnvironmentSensors};

/// 環境センサーの実装を返す（実体は #612）。
pub(crate) fn implementation() -> Arc<dyn EnvironmentSensors> {
    Arc::new(UnsupportedEnvironmentSensors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::environment_sensors::{EnvSensorFailure, SensorKind};

    #[test]
    fn stub_never_succeeds() {
        let imp = implementation();
        for k in SensorKind::ALL {
            assert_eq!(imp.start(k), Err(EnvSensorFailure::UnsupportedOnOs));
            assert_eq!(imp.read(k), Err(EnvSensorFailure::UnsupportedOnOs));
        }
    }
}
