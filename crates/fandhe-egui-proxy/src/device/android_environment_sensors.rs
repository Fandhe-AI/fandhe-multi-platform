//! Android 向け気圧・照度・近接センサー実装のスタブ（Issue #609、親 #123）。`device/environment_sensors.rs` の
//! Android 経路の dispatch 先で、後続の子 #611 が実体を入れるまで全種類で `UnsupportedOnOs` を返す
//! （ダミーの成功値へは倒さない）。
//!
//! 後続が埋める範囲: Java ヘルパーと同期 JNI の署名を Java 側・no-op スタブ・実体の 3 か所で一致させる。
//! `onPause` / `onResume` / `onDestroy` を扱い、気圧だけ `is_fresh` で古さを判定、近接は
//! `proximity_from_distance` で 2 値にする。
//! 後続は本ファイル内の `#[cfg(target_os = "android")]` の項目と OS 固有の付帯物（Java・`AndroidManifest`）だけを触る。
//! dispatch・画面・fallback 行・`device.rs` の `pub mod` 列は触らない。

use std::sync::Arc;

use fandhe_device::environment_sensors::{EnvironmentSensors, UnsupportedEnvironmentSensors};

/// 環境センサーの実装を返す（実体は #611）。
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
