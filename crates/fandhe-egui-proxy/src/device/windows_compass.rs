//! Windows 向け方位実装（Issue #1404、親 #1154）。`device/compass.rs` の Windows 経路から
//! UI スレッドで呼ばれ、WinRT `Windows.Devices.Sensors.Compass` の最新値を同期に pull する
//! （`ReadingChanged` は購読しない。`Compass` トレイトを実装）。
//!
//! # 流れ
//!
//! - `start`: `Compass::GetDefault()`。null（`e.code().0 == 0`）は `Unsupported`、`E_ACCESSDENIED` は
//!   `PermissionDenied`、他は `Failed`。`ReportInterval` を `max(MinimumReportInterval, 100 ms)` に
//!   する。設定に失敗したら 0 へ戻して保持せず `Failed`（類型 9）
//! - `read`: `GetCurrentReading()`。null は値なし。`HeadingMagneticNorth` を `normalize_heading` に
//!   通し、範囲外は値なし。精度の取得失敗は `Unknown`（方位自体は有効）
//! - `stop`: `ReportInterval` を 0 に戻して保持を捨てる（結果は無視）。冪等
//!
//! `HeadingTrueNorth` と `ReadingTransform` は読まない・設定しない（磁北のみ、画面回転の補正なし）。
//! HRESULT・方位・精度はログにも画面にも出さない（security.md A09）。`unsafe` と `RoInitialize` は
//! 使わない（既存の Windows 実装群と同じ前提）。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.compass>
//!
//! # 既知の天井
//!
//! - パッケージ無しの素の exe から届くか、プライバシー設定で拒否されたときの HRESULT は実機未確認
//!   （#1403）。届かないと判明したら対応表の `done` を取り下げる
//! - 値は 2° 刻み。Connected Standby 中は null が返りうる（値なし扱い）
//! - ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

use fandhe_device::compass::{Accuracy, CompassFailure};

/// `E_ACCESSDENIED`（0x80070005）。権限拒否の写像に使う。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

/// `MagnetometerAccuracy`（Unknown = 0 / Unreliable = 1 / Approximate = 2 / High = 3）の写像。
/// 範囲外は `Unknown`。
fn accuracy_from_raw(raw: i32) -> Accuracy {
    match raw {
        1 => Accuracy::Unreliable,
        2 => Accuracy::Approximate,
        3 => Accuracy::High,
        _ => Accuracy::Unknown,
    }
}

/// `GetDefault` / `SetReportInterval` の失敗 HRESULT を契約の失敗へ写す。0（S_OK + null）は
/// 搭載センサー無し。
fn start_failure_from_hresult(code: i32) -> CompassFailure {
    match code {
        0 => CompassFailure::Unsupported,
        E_ACCESSDENIED => CompassFailure::PermissionDenied,
        _ => CompassFailure::Failed,
    }
}

/// `GetCurrentReading` の失敗 HRESULT を写す。0（null の読み取り）は値なしで `None`。
fn read_failure_from_hresult(code: i32) -> Option<CompassFailure> {
    match code {
        0 => None,
        E_ACCESSDENIED => Some(CompassFailure::PermissionDenied),
        _ => Some(CompassFailure::Failed),
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::{Arc, Mutex};

    use windows::Devices::Sensors::Compass as WinCompass;

    use super::*;
    use fandhe_device::compass::{normalize_heading, Compass, Heading};

    /// 保持している `Compass`（プロセスで 1 個）。
    pub(super) struct WindowsCompass {
        sensor: Mutex<Option<WinCompass>>,
    }

    pub(crate) fn implementation() -> Arc<dyn Compass> {
        Arc::new(WindowsCompass {
            sensor: Mutex::new(None),
        })
    }

    fn lock(m: &Mutex<Option<WinCompass>>) -> std::sync::MutexGuard<'_, Option<WinCompass>> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    impl Compass for WindowsCompass {
        fn start(&self) -> Result<(), CompassFailure> {
            let mut guard = lock(&self.sensor);
            if guard.is_some() {
                return Ok(());
            }
            let sensor =
                WinCompass::GetDefault().map_err(|e| start_failure_from_hresult(e.code().0))?;
            let interval = sensor
                .MinimumReportInterval()
                .map_err(|e| start_failure_from_hresult(e.code().0))?
                .max(100);
            if let Err(e) = sensor.SetReportInterval(interval) {
                // 設定に失敗したら元へ戻し、保持しない。
                let _ = sensor.SetReportInterval(0);
                return Err(start_failure_from_hresult(e.code().0));
            }
            *guard = Some(sensor);
            Ok(())
        }

        fn read(&self) -> Result<Option<Heading>, CompassFailure> {
            let guard = lock(&self.sensor);
            let Some(sensor) = guard.as_ref() else {
                return Ok(None);
            };
            let reading = match sensor.GetCurrentReading() {
                Ok(r) => r,
                Err(e) => return read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
            };
            let raw = match reading.HeadingMagneticNorth() {
                Ok(v) => v,
                Err(e) => return read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
            };
            let Some(degrees) = normalize_heading(raw) else {
                return Ok(None);
            };
            let accuracy = reading
                .HeadingAccuracy()
                .map_or(Accuracy::Unknown, |a| accuracy_from_raw(a.0));
            Ok(Some(Heading { degrees, accuracy }))
        }

        fn stop(&self) {
            if let Some(sensor) = lock(&self.sensor).take() {
                let _ = sensor.SetReportInterval(0);
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
    fn accuracy_maps_one_to_one_and_unknown_when_out_of_range() {
        assert_eq!(accuracy_from_raw(0), Accuracy::Unknown);
        assert_eq!(accuracy_from_raw(1), Accuracy::Unreliable);
        assert_eq!(accuracy_from_raw(2), Accuracy::Approximate);
        assert_eq!(accuracy_from_raw(3), Accuracy::High);
        assert_eq!(accuracy_from_raw(4), Accuracy::Unknown);
        assert_eq!(accuracy_from_raw(-1), Accuracy::Unknown);
    }

    #[test]
    fn hresult_mapping_never_yields_success() {
        assert_eq!(start_failure_from_hresult(0), CompassFailure::Unsupported);
        assert_eq!(
            start_failure_from_hresult(E_ACCESSDENIED),
            CompassFailure::PermissionDenied
        );
        assert_eq!(start_failure_from_hresult(-1), CompassFailure::Failed);
        assert_eq!(read_failure_from_hresult(0), None);
        assert_eq!(
            read_failure_from_hresult(E_ACCESSDENIED),
            Some(CompassFailure::PermissionDenied)
        );
        assert_eq!(read_failure_from_hresult(-1), Some(CompassFailure::Failed));
    }
}
