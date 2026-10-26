//! Windows 向けジャイロ・磁気・線形加速度の生値実装（Issue #899、親 #180）。
//! `device/motion_raw_sensors.rs` の Windows 経路から UI スレッドで呼ばれ、WinRT
//! `Windows.Devices.Sensors` の最新値を同期に pull する（`ReadingChanged` は購読しない。
//! `MotionRawSensors` トレイトを実装）。契約は `fandhe_device::motion_raw_sensors` の doc に従う。
//!
//! # 流れ
//!
//! - `start`: 種類ごとに `Gyrometer` / `Magnetometer` / `Accelerometer` の `GetDefault` を取る。
//!   null（`e.code().0 == 0`）は `NotPresent`、`E_ACCESSDENIED` は `PermissionDenied`、
//!   `E_NOINTERFACE`（`IAccelerometerStatics2` が無い Windows 10 1607 未満）は `UnsupportedOnOs`、
//!   他は `Failed`。`ReportInterval` は `max(MinimumReportInterval, 100 ms)`。設定に失敗したら
//!   0 へ戻して保持せず失敗（類型 9）
//! - `read`: `GetCurrentReading()` の 3 軸。null は値なし。3 軸がそろい有限なときだけ値にし、
//!   1 軸でも取れなければ値なし／失敗（0 や前回値で埋めない）
//! - `stop`: 保持を捨て `ReportInterval` を 0 に戻す（結果は無視）。冪等
//!
//! 線形加速度は `AccelerometerReadingType::Linear` で取る（重力は OS が除く。#898 で「#899 で
//! 確定する」とされた点）。単位は OS のまま（度/秒・µT・g）。`MagnetometerReading` の
//! `DirectionalAccuracy` と `Timestamp` は読まない（較正は範囲外。古い値の破棄は Windows が返す
//! null だけに任せ、時刻による古さの閾値は置かない）。最小化中は eframe が `update` を呼ばず
//! driver が読まない。HRESULT・センサー値はログにも画面にも出さない（security.md A09）。
//! `unsafe` と `RoInitialize` は使わない。
//!
//! 一次情報:
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.gyrometer> /
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.magnetometer> /
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.accelerometer.getdefault> /
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.accelerometerreadingtype>
//!
//! # 既知の天井
//!
//! - パッケージ無しの素の exe から届くか、拒否時の HRESULT は実機未確認（#885）。届かないと
//!   判明したら対応表の `done` を取り下げる
//! - `GetCurrentReading` が `ReadingChanged` 無しで更新されるかも実機未確認（compass と同じ前提）
//! - ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

use fandhe_device::motion_raw_sensors::{
    validate_reading, RawKind, RawReading, RawSensorFailure, RawUnit,
};

/// `E_ACCESSDENIED`（0x80070005）。権限拒否の写像に使う。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
/// `E_NOINTERFACE`（0x80004002）。OS に当該 API の statics が無いことの写像に使う。
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;

/// `GetDefault` / `MinimumReportInterval` / `SetReportInterval` の失敗 HRESULT を契約の失敗へ写す。
/// 0（S_OK + null）は搭載センサー無し。
fn start_failure_from_hresult(code: i32) -> RawSensorFailure {
    match code {
        0 => RawSensorFailure::NotPresent,
        E_ACCESSDENIED => RawSensorFailure::PermissionDenied,
        E_NOINTERFACE => RawSensorFailure::UnsupportedOnOs,
        _ => RawSensorFailure::Failed,
    }
}

/// `GetCurrentReading` と各軸取得の失敗 HRESULT を写す。0（null の読み取り）は値なしで `None`。
fn read_failure_from_hresult(code: i32) -> Option<RawSensorFailure> {
    match code {
        0 => None,
        E_ACCESSDENIED => Some(RawSensorFailure::PermissionDenied),
        _ => Some(RawSensorFailure::Failed),
    }
}

/// OS が出す単位（Gyrometer は度/秒、Magnetometer は µT、Accelerometer は g）。
fn unit_of(kind: RawKind) -> RawUnit {
    match kind {
        RawKind::AngularVelocity => RawUnit::DegPerSec,
        RawKind::MagneticField => RawUnit::MicroTesla,
        RawKind::LinearAcceleration => RawUnit::StandardGravity,
    }
}

/// 3 軸に単位を付けて検査する。非有限は値なし。
fn to_reading(kind: RawKind, x: f64, y: f64, z: f64) -> Option<RawReading> {
    validate_reading(
        kind,
        RawReading {
            x,
            y,
            z,
            unit: unit_of(kind),
        },
    )
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::{Arc, Mutex};

    use windows::Devices::Sensors::{
        Accelerometer, AccelerometerReadingType, Gyrometer, Magnetometer,
    };

    use super::*;
    use fandhe_device::motion_raw_sensors::MotionRawSensors;

    /// 保持しているセンサー。
    enum Held {
        Gyro(Gyrometer),
        Mag(Magnetometer),
        Lin(Accelerometer),
    }

    /// `RawKind::ALL` の添字ごとに高々 1 個保持する（種類ごとに購読を二重にしない）。
    struct WindowsMotionRawSensors {
        held: Mutex<[Option<Held>; 3]>,
    }

    pub(crate) fn implementation() -> Arc<dyn MotionRawSensors> {
        Arc::new(WindowsMotionRawSensors {
            held: Mutex::new([None, None, None]),
        })
    }

    fn idx(kind: RawKind) -> usize {
        match kind {
            RawKind::AngularVelocity => 0,
            RawKind::MagneticField => 1,
            RawKind::LinearAcceleration => 2,
        }
    }

    fn fail(e: windows::core::Error) -> RawSensorFailure {
        start_failure_from_hresult(e.code().0)
    }

    /// `$s` に `ReportInterval`（最小値と 100 ms の大きい方）を設定する。
    /// 設定に失敗したら 0 へ戻してエラーを返す。
    macro_rules! set_interval {
        ($s:expr) => {{
            let min = $s.MinimumReportInterval().map_err(fail)?;
            if let Err(e) = $s.SetReportInterval(min.max(100)) {
                let _ = $s.SetReportInterval(0);
                return Err(fail(e));
            }
        }};
    }

    fn acquire(kind: RawKind) -> Result<Held, RawSensorFailure> {
        Ok(match kind {
            RawKind::AngularVelocity => {
                let s = Gyrometer::GetDefault().map_err(fail)?;
                set_interval!(s);
                Held::Gyro(s)
            }
            RawKind::MagneticField => {
                let s = Magnetometer::GetDefault().map_err(fail)?;
                set_interval!(s);
                Held::Mag(s)
            }
            RawKind::LinearAcceleration => {
                let s = Accelerometer::GetDefaultWithAccelerometerReadingType(
                    AccelerometerReadingType::Linear,
                )
                .map_err(fail)?;
                set_interval!(s);
                Held::Lin(s)
            }
        })
    }

    /// 取得失敗を契約の「値なし／失敗」へ写す。
    fn rd<T>(r: windows::core::Result<T>) -> Result<Option<T>, RawSensorFailure> {
        match r {
            Ok(v) => Ok(Some(v)),
            Err(e) => read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
        }
    }

    fn read_axes(h: &Held) -> Result<Option<(f64, f64, f64)>, RawSensorFailure> {
        Ok(match h {
            Held::Gyro(s) => {
                let Some(r) = rd(s.GetCurrentReading())? else {
                    return Ok(None);
                };
                let (Some(x), Some(y), Some(z)) = (
                    rd(r.AngularVelocityX())?,
                    rd(r.AngularVelocityY())?,
                    rd(r.AngularVelocityZ())?,
                ) else {
                    return Ok(None);
                };
                Some((x, y, z))
            }
            Held::Mag(s) => {
                let Some(r) = rd(s.GetCurrentReading())? else {
                    return Ok(None);
                };
                let (Some(x), Some(y), Some(z)) = (
                    rd(r.MagneticFieldX())?,
                    rd(r.MagneticFieldY())?,
                    rd(r.MagneticFieldZ())?,
                ) else {
                    return Ok(None);
                };
                Some((f64::from(x), f64::from(y), f64::from(z)))
            }
            Held::Lin(s) => {
                let Some(r) = rd(s.GetCurrentReading())? else {
                    return Ok(None);
                };
                let (Some(x), Some(y), Some(z)) = (
                    rd(r.AccelerationX())?,
                    rd(r.AccelerationY())?,
                    rd(r.AccelerationZ())?,
                ) else {
                    return Ok(None);
                };
                Some((x, y, z))
            }
        })
    }

    fn release(h: Held) {
        let _ = match h {
            Held::Gyro(s) => s.SetReportInterval(0),
            Held::Mag(s) => s.SetReportInterval(0),
            Held::Lin(s) => s.SetReportInterval(0),
        };
    }

    impl MotionRawSensors for WindowsMotionRawSensors {
        fn start(&self, kind: RawKind) -> Result<(), RawSensorFailure> {
            let mut guard = self.held.lock().unwrap_or_else(|p| p.into_inner());
            if guard[idx(kind)].is_some() {
                return Ok(());
            }
            guard[idx(kind)] = Some(acquire(kind)?);
            Ok(())
        }

        fn read(&self, kind: RawKind) -> Result<Option<RawReading>, RawSensorFailure> {
            let guard = self.held.lock().unwrap_or_else(|p| p.into_inner());
            let Some(h) = guard[idx(kind)].as_ref() else {
                return Ok(None);
            };
            Ok(read_axes(h)?.and_then(|(x, y, z)| to_reading(kind, x, y, z)))
        }

        fn stop(&self, kind: RawKind) {
            let taken = self.held.lock().unwrap_or_else(|p| p.into_inner())[idx(kind)].take();
            if let Some(h) = taken {
                release(h);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use imp::implementation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hresult_mapping_never_yields_success() {
        assert_eq!(start_failure_from_hresult(0), RawSensorFailure::NotPresent);
        assert_eq!(
            start_failure_from_hresult(E_ACCESSDENIED),
            RawSensorFailure::PermissionDenied
        );
        assert_eq!(
            start_failure_from_hresult(E_NOINTERFACE),
            RawSensorFailure::UnsupportedOnOs
        );
        assert_eq!(start_failure_from_hresult(-1), RawSensorFailure::Failed);
        assert_eq!(read_failure_from_hresult(0), None);
        assert_eq!(
            read_failure_from_hresult(E_ACCESSDENIED),
            Some(RawSensorFailure::PermissionDenied)
        );
        assert_eq!(
            read_failure_from_hresult(-1),
            Some(RawSensorFailure::Failed)
        );
    }

    #[test]
    fn unit_matches_os_and_kind() {
        for k in RawKind::ALL {
            assert!(unit_of(k).fits(k));
        }
        assert_eq!(unit_of(RawKind::AngularVelocity), RawUnit::DegPerSec);
        assert_eq!(unit_of(RawKind::MagneticField), RawUnit::MicroTesla);
        assert_eq!(
            unit_of(RawKind::LinearAcceleration),
            RawUnit::StandardGravity
        );
    }

    #[test]
    fn non_finite_axis_is_no_value() {
        for k in RawKind::ALL {
            for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert_eq!(to_reading(k, bad, 0.0, 0.0), None);
                assert_eq!(to_reading(k, 0.0, bad, 0.0), None);
                assert_eq!(to_reading(k, 0.0, 0.0, bad), None);
            }
            let r = to_reading(k, 1.0, -2.0, 3.0).expect("有限値は値になる");
            assert_eq!(r.unit, unit_of(k));
        }
    }
}
