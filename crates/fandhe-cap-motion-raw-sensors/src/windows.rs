//! Windows 向けジャイロ・磁気・線形加速度の生値実装（Issue #1145、親 #180）。
//! 本 crate の `driver` の Windows 経路の dispatch 先で、UI スレッドの driver から種類ごとに
//! `start` / `read` / `stop` を呼ばれる。契約は `crate::contract`（特に
//! 「後続の OS 実装への要点 > Windows」）に従う。
//!
//! # 流れ
//!
//! - `start`: 最小化ウォッチャー（`windows_environment_sensors` と共用）を確保できなければ `Failed`。
//!   その後 `Gyrometer` / `Magnetometer` / `Accelerometer`（`AccelerometerReadingType::Linear`）の
//!   `GetDefault` を行い保持する。null は `NotPresent`、`E_ACCESSDENIED` は `PermissionDenied`、他は `Failed`
//! - `read`: `GetCurrentReading` と 3 軸を pull する。null の読みは値なし。軸の取得失敗は 0 や前回値で
//!   埋めず `Err`。単位は OS のまま（°/s・µT・g）で、`validate_reading` を通して非有限は値なし
//! - `stop`: 保持を drop するだけ（冪等）。`ReportInterval` は設定しないので戻す処理も解放の失敗経路も無い
//!
//! # 背景遷移（最小化）
//!
//! 共通層のウォッチャー（`fandhe_platform_windows::minimize`）へ登録した受け口が、最小化で `note_minimized`（保持を全て drop）、復元で `note_restored`（`wanted` の種類に
//! 「取り直し要」の印を立てるだけ。OS は呼ばない）を呼ぶ。`GetDefault` は復元後の UI スレッドの
//! 次の `read` が行い、失敗はその `read` の `Err` になる。非表示中の `read` は `Ok(None)`。
//! 判定と更新は 1 本の `Mutex` の内側で行う。
//!
//! HRESULT・センサー値はログにも画面にも出さない（security.md A09）。新しい `unsafe` は無い。
//!
//! 一次情報:
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.gyrometer>、
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.magnetometer>、
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.accelerometer.getdefault>、
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.accelerometerreadingtype>
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 嘘の値（0・前回値・非有限・最小化後の古い保持）を出さない、失敗種別を取り違えない、
//! 停止と最小化で保持を捨てる、判定と更新の原子性。
//! 守らない範囲: 絶対精度、OS 間で値がそろうこと、最小化の検出の遅れ（最大 200 ms）、
//! Connected Standby、複数プロセス。
//!
//! # 既知の天井
//!
//! - センサーオブジェクトを UI 以外のスレッドで drop してよいか、既定の間隔で値が返るか、
//!   素の exe での HRESULT は実機未確認（#885）
//! - ローカルの型検査は `x86_64-pc-windows-gnu` のみ

use crate::contract::{RawKind, RawReading, RawSensorFailure, RawUnit, validate_reading};

/// `E_ACCESSDENIED`（0x80070005）。権限拒否の写像に使う。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

/// 開始系（`GetDefault`）の失敗 HRESULT を契約の失敗へ写す。0（S_OK + null）は端末にセンサーが無い。
/// Windows 10 から公開 API があるので `UnsupportedOnOs` は返さない。
#[cfg_attr(not(windows), allow(dead_code))]
fn start_failure_from_hresult(code: i32) -> RawSensorFailure {
    match code {
        0 => RawSensorFailure::NotPresent,
        E_ACCESSDENIED => RawSensorFailure::PermissionDenied,
        _ => RawSensorFailure::Failed,
    }
}

/// 読み取り系の失敗 HRESULT を写す。0（null の読み）は値なしで `None`。
#[cfg_attr(not(windows), allow(dead_code))]
fn read_failure_from_hresult(code: i32) -> Option<RawSensorFailure> {
    match code {
        0 => None,
        E_ACCESSDENIED => Some(RawSensorFailure::PermissionDenied),
        _ => Some(RawSensorFailure::Failed),
    }
}

/// 種類ごとの OS 単位（ジャイロ °/s、磁気 µT、線形加速度 g）。
fn unit_of(kind: RawKind) -> RawUnit {
    match kind {
        RawKind::AngularVelocity => RawUnit::DegPerSec,
        RawKind::MagneticField => RawUnit::MicroTesla,
        RawKind::LinearAcceleration => RawUnit::StandardGravity,
    }
}

/// 3 軸の値を単位付きの読みにする。非有限は値なし。
#[cfg_attr(not(windows), allow(dead_code))]
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

fn index(kind: RawKind) -> usize {
    RawKind::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or_default()
}

/// 種類ごとの保持状態。
struct KindSlot<T> {
    /// 利用者が購読を望んでいる。
    wanted: bool,
    /// OS から取得済みのセンサー。
    held: Option<T>,
    /// 復元後に次の `read` で取り直す。
    reacquire: bool,
}

impl<T> Default for KindSlot<T> {
    fn default() -> Self {
        Self {
            wanted: false,
            held: None,
            reacquire: false,
        }
    }
}

/// 全種類の保持状態と非表示フラグ。OS 呼び出しは `acquire` として注入し、host でテストできる。
struct Slots<T> {
    suspended: bool,
    kinds: [KindSlot<T>; 3],
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self {
            suspended: false,
            kinds: Default::default(),
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
impl<T: Clone> Slots<T> {
    fn start<F>(&mut self, kind: RawKind, acquire: impl FnOnce() -> Result<T, F>) -> Result<(), F> {
        let suspended = self.suspended;
        let s = &mut self.kinds[index(kind)];
        if s.wanted && (s.held.is_some() || s.reacquire) {
            return Ok(());
        }
        if suspended {
            // 最小化中は OS のセンサーを握らない。復元後の `read` で取得する。
            s.wanted = true;
            s.reacquire = true;
            return Ok(());
        }
        s.held = Some(acquire()?);
        s.wanted = true;
        s.reacquire = false;
        Ok(())
    }

    fn stop(&mut self, kind: RawKind) {
        self.kinds[index(kind)] = KindSlot::default();
    }

    fn minimize(&mut self) {
        self.suspended = true;
        for s in &mut self.kinds {
            s.held = None;
        }
    }

    fn restore(&mut self) {
        self.suspended = false;
        for s in &mut self.kinds {
            s.reacquire = s.wanted;
        }
    }

    /// `read` の前段。必要なら取り直して、読むべき保持を返す。
    fn sensor_for_read<F>(
        &mut self,
        kind: RawKind,
        acquire: impl FnOnce() -> Result<T, F>,
    ) -> Result<Option<T>, F> {
        let suspended = self.suspended;
        let s = &mut self.kinds[index(kind)];
        if !s.wanted || suspended {
            return Ok(None);
        }
        if s.reacquire {
            s.reacquire = false;
            match acquire() {
                Ok(v) => s.held = Some(v),
                Err(e) => {
                    s.held = None;
                    return Err(e);
                }
            }
        }
        Ok(s.held.clone())
    }

    /// 読み取り中に最小化・停止されていないか。
    fn is_live(&self, kind: RawKind) -> bool {
        !self.suspended && self.kinds[index(kind)].held.is_some()
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::{Arc, Mutex, MutexGuard};

    use windows::Devices::Sensors::{
        Accelerometer, AccelerometerReadingType, Gyrometer, Magnetometer,
    };

    use super::*;
    use crate::contract::MotionRawSensors;
    use fandhe_platform_windows::minimize::{MinimizeEdge, watch_minimize};

    #[derive(Clone)]
    enum Held {
        Gyro(Gyrometer),
        Mag(Magnetometer),
        Lin(Accelerometer),
    }

    static STATE: Mutex<Option<Slots<Held>>> = Mutex::new(None);

    fn state() -> MutexGuard<'static, Option<Slots<Held>>> {
        STATE.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn with<R>(f: impl FnOnce(&mut Slots<Held>) -> R) -> R {
        f(state().get_or_insert_with(Slots::default))
    }

    fn acquire(kind: RawKind) -> Result<Held, RawSensorFailure> {
        let fail = |e: windows::core::Error| start_failure_from_hresult(e.code().0);
        match kind {
            RawKind::AngularVelocity => Gyrometer::GetDefault().map(Held::Gyro).map_err(fail),
            RawKind::MagneticField => Magnetometer::GetDefault().map(Held::Mag).map_err(fail),
            RawKind::LinearAcceleration => Accelerometer::GetDefaultWithAccelerometerReadingType(
                AccelerometerReadingType::Linear,
            )
            .map(Held::Lin)
            .map_err(fail),
        }
    }

    /// 読みの失敗 HRESULT を写す。`Ok(None)` は null の読み。
    fn rd<T>(r: windows::core::Result<T>) -> Result<Option<T>, RawSensorFailure> {
        match r {
            Ok(v) => Ok(Some(v)),
            Err(e) => read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
        }
    }

    fn read_axes(held: &Held) -> Result<Option<(f64, f64, f64)>, RawSensorFailure> {
        // 軸が 1 つでも取れなければ 0 で埋めず、同じ写像で値なし / `Err` にする。
        macro_rules! axes {
            ($r:expr, $x:ident, $y:ident, $z:ident) => {{
                let Some(r) = rd($r)? else { return Ok(None) };
                let (Some(x), Some(y), Some(z)) = (rd(r.$x())?, rd(r.$y())?, rd(r.$z())?) else {
                    return Ok(None);
                };
                Ok(Some((x as f64, y as f64, z as f64)))
            }};
        }
        match held {
            Held::Gyro(s) => axes!(
                s.GetCurrentReading(),
                AngularVelocityX,
                AngularVelocityY,
                AngularVelocityZ
            ),
            Held::Mag(s) => axes!(
                s.GetCurrentReading(),
                MagneticFieldX,
                MagneticFieldY,
                MagneticFieldZ
            ),
            Held::Lin(s) => axes!(
                s.GetCurrentReading(),
                AccelerationX,
                AccelerationY,
                AccelerationZ
            ),
        }
    }

    struct WindowsMotionRawSensors;

    impl MotionRawSensors for WindowsMotionRawSensors {
        fn start(&self, kind: RawKind) -> Result<(), RawSensorFailure> {
            if !watch_minimize(on_minimize) {
                return Err(RawSensorFailure::Failed);
            }
            with(|s| s.start(kind, || acquire(kind)))
        }

        fn read(&self, kind: RawKind) -> Result<Option<RawReading>, RawSensorFailure> {
            let Some(held) = with(|s| s.sensor_for_read(kind, || acquire(kind)))? else {
                return Ok(None);
            };
            let axes = read_axes(&held)?;
            // 読み取り中に最小化・停止されたら古い値を出さない。
            if !with(|s| s.is_live(kind)) {
                return Ok(None);
            }
            Ok(axes.and_then(|(x, y, z)| to_reading(kind, x, y, z)))
        }

        fn stop(&self, kind: RawKind) {
            with(|s| s.stop(kind));
        }
    }

    pub(crate) fn implementation() -> Arc<dyn MotionRawSensors> {
        Arc::new(WindowsMotionRawSensors)
    }

    /// 共通層のウォッチャーへ登録する受け口。最小化で保持を全て drop し、復元で取り直しの印を立てる。
    fn on_minimize(edge: MinimizeEdge) {
        match edge {
            MinimizeEdge::Minimized => with(|s| s.minimize()),
            MinimizeEdge::Restored => with(|s| s.restore()),
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use imp::implementation;

#[cfg(test)]
mod tests {
    use super::*;

    const G: RawKind = RawKind::AngularVelocity;

    fn ok(v: u32) -> impl FnOnce() -> Result<u32, RawSensorFailure> {
        move || Ok(v)
    }

    fn never() -> Result<u32, RawSensorFailure> {
        panic!("OS を呼んではいけない")
    }

    #[test]
    fn hresult_never_succeeds() {
        assert_eq!(start_failure_from_hresult(0), RawSensorFailure::NotPresent);
        assert_eq!(
            start_failure_from_hresult(E_ACCESSDENIED),
            RawSensorFailure::PermissionDenied
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
    fn units_match_kind_and_non_finite_dropped() {
        for k in RawKind::ALL {
            assert_eq!(to_reading(k, 1.0, 2.0, 3.0).unwrap().unit, unit_of(k));
            assert!(to_reading(k, f64::NAN, 0.0, 0.0).is_none());
            assert!(to_reading(k, 0.0, f64::INFINITY, 0.0).is_none());
        }
    }

    #[test]
    fn start_failure_holds_nothing_and_start_is_idempotent() {
        let mut s = Slots::<u32>::default();
        assert_eq!(
            s.start(G, || Err(RawSensorFailure::NotPresent)),
            Err(RawSensorFailure::NotPresent)
        );
        assert_eq!(s.sensor_for_read(G, ok(1)), Ok(None));
        assert!(s.start(G, ok(1)).is_ok());
        assert!(s.start(G, never).is_ok());
    }

    #[test]
    fn minimize_drops_and_restore_reacquires_once() {
        let mut s = Slots::<u32>::default();
        s.start(G, ok(1)).unwrap();
        s.minimize();
        assert!(!s.is_live(G));
        assert_eq!(s.sensor_for_read(G, never), Ok(None));
        s.restore();
        assert_eq!(s.sensor_for_read(G, ok(2)), Ok(Some(2)));
        assert_eq!(s.sensor_for_read(G, never), Ok(Some(2)));
    }

    #[test]
    fn reacquire_failure_is_err_not_stale() {
        let mut s = Slots::<u32>::default();
        s.start(G, ok(1)).unwrap();
        s.minimize();
        s.restore();
        assert_eq!(
            s.sensor_for_read(G, || Err(RawSensorFailure::Failed)),
            Err(RawSensorFailure::Failed)
        );
        assert!(!s.is_live(G));
    }

    #[test]
    fn stop_while_suspended_cancels_reacquire() {
        let mut s = Slots::<u32>::default();
        s.start(G, ok(1)).unwrap();
        s.minimize();
        s.stop(G);
        s.restore();
        assert_eq!(s.sensor_for_read(G, never), Ok(None));
    }

    #[test]
    fn start_while_suspended_defers_to_read() {
        let mut s = Slots::<u32>::default();
        s.minimize();
        assert!(s.start(G, never).is_ok());
        s.restore();
        assert_eq!(s.sensor_for_read(G, ok(7)), Ok(Some(7)));
    }

    #[test]
    fn stop_then_read_is_none() {
        let mut s = Slots::<u32>::default();
        s.start(G, ok(1)).unwrap();
        s.stop(G);
        assert_eq!(s.sensor_for_read(G, ok(1)), Ok(None));
    }
}
