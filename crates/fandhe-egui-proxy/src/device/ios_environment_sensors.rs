//! iOS 向け気圧・照度・近接センサー実装のスタブ（Issue #609、親 #123）。`device/environment_sensors.rs` の
//! iOS 経路の dispatch 先で、後続の子 #610 が実体を入れるまで全種類で `UnsupportedOnOs` を返す
//! （ダミーの成功値へは倒さない）。
//!
//! 後続が埋める範囲: 気圧は `CMAltimeter`（kPa を `pressure_hpa_from_kpa` で換算）、近接は `UIDevice` の
//! proximity monitoring。照度は公開 API が無いので常に `UnsupportedOnOs`。拒否コードは 2 種とも拾い、
//! main queue で切り替えて `dispatch_sync` は使わず、背景遷移で値を捨てる
//! （`device/environment_sensors.rs` の `note_backgrounded` / `note_resumed` を呼ぶ）。
//! 後続は本ファイル内の `#[cfg(target_os = "ios")]` の項目と OS 固有の付帯物（`.m`・build.rs・Info.plist）だけを触る。
//! dispatch・画面・fallback 行・`device.rs` の `pub mod` 列は触らない。

use std::sync::Arc;

use fandhe_device::environment_sensors::{EnvironmentSensors, UnsupportedEnvironmentSensors};

/// 環境センサーの実装を返す（実体は #610）。
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
