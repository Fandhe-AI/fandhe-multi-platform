//! iOS 向けジャイロ・磁気・線形加速度の生値実装の骨格（Issue #898、親 #180）。
//! `device/motion_raw_sensors.rs` の iOS 経路の dispatch 先。実体は #900 が、このファイルと
//! iOS 固有の付帯物だけを置き換えて埋める。API・単位・背景遷移の扱いは
//! `fandhe_device::motion_raw_sensors` のモジュール doc「後続の OS 実装への要点」に従う。
//! iOS の `g_motion_manager` の共有は #900 が行う。
//!
//! 後続が触らないもの: dispatch（`device/motion_raw_sensors.rs`）・共通コア画面・fallback 行・
//! `device.rs` の `pub mod` 列。実体が入るまでは `Unsupported` を返し、成功を偽らない。

use std::sync::Arc;

use fandhe_device::motion_raw_sensors::{MotionRawSensors, UnsupportedMotionRawSensors};

/// iOS の `MotionRawSensors` 実装を返す（骨格のため `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn MotionRawSensors> {
    Arc::new(UnsupportedMotionRawSensors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::motion_raw_sensors::{RawKind, RawSensorFailure};

    #[test]
    fn stub_never_succeeds() {
        let imp = implementation();
        for k in RawKind::ALL {
            assert_eq!(imp.start(k), Err(RawSensorFailure::UnsupportedOnOs));
            assert_eq!(imp.read(k), Err(RawSensorFailure::UnsupportedOnOs));
        }
    }
}
