//! Phase 11 の拡充ループ（親 #180、Issue #898）「ジャイロ・磁気・線形加速度」の effect ドライバ。
//! `fandhe_core::screens::motion_raw_sensors_show`（端末依存ゼロ）の種類ごとの `wanted` の立ち上がりで
//! `fandhe_device::motion_raw_sensors::MotionRawSensors::start`、立ち下がりで `stop` を呼び、購読中は
//! [`READ_INTERVAL`] ごとに `read` して結果を注入する（`registry.rs` の `effect` 区画から
//! 毎フレーム同期に呼ばれる）。`environment_sensors.rs` と同型の間隔つき pull を種類ごとに回す。
//!
//! **間隔つき pull**: 最終読取から `READ_INTERVAL` 未満なら OS を読まず残り時間で再描画を予約する。
//! 読んだら送信を省く分岐でも次回を必ず予約する（契約表 4）。待機（購読なし）では予約しない。
//! 期限（値が届かない場合の失敗化）と背景遷移の処理は持たない。背景遷移は各 OS の実装が OS の事実で扱う。
//!
//! 失敗（開始失敗・読み取り失敗）を確定する前に必ず `stop(kind)` する（契約表 3・9）。
//! 完了は読み取り時の `session` を持たせ、古い世代の完了は画面側で捨てられる。
//! 1 つの種類の失敗で他の種類を止めない。判定と状態更新は `DRIVER` のロックの内側で行い、
//! 経路はこのドライバ 1 本に集約する（契約表 2・11）。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS・Android・Windows は各 OS モジュール
//! （#900・#901・#899）、macOS・Linux・wasm32 は `UnsupportedMotionRawSensors` 固定。
//! Web は `fandhe-web-dom`（#902）が配線する。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use fandhe_core::screens::motion_raw_sensors_show::{
    apply_failure, apply_reading, display_equal, MotionRawSensorsShowState, RawFailureView,
    RawKindView, RawReadingView, RawSensorView, RawUnitView,
};
#[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
use fandhe_device::motion_raw_sensors::UnsupportedMotionRawSensors;
use fandhe_device::motion_raw_sensors::{
    validate_reading, MotionRawSensors, RawKind, RawReading, RawSensorFailure, RawUnit,
    READ_INTERVAL,
};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す（プロセスで 1 個。購読を二重に作らないため
/// `OnceLock` で保持する）。
fn implementation() -> Arc<dyn MotionRawSensors> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn MotionRawSensors>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                super::windows_motion_raw_sensors::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_motion_raw_sensors::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_motion_raw_sensors::implementation()
            }
            #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
            {
                Arc::new(UnsupportedMotionRawSensors)
            }
        })
        .clone()
}

fn to_kind_view(k: RawKind) -> RawKindView {
    match k {
        RawKind::AngularVelocity => RawKindView::AngularVelocity,
        RawKind::MagneticField => RawKindView::MagneticField,
        RawKind::LinearAcceleration => RawKindView::LinearAcceleration,
    }
}

fn to_view(r: RawReading) -> RawReadingView {
    RawReadingView {
        x: r.x,
        y: r.y,
        z: r.z,
        unit: match r.unit {
            RawUnit::RadPerSec => RawUnitView::RadPerSec,
            RawUnit::DegPerSec => RawUnitView::DegPerSec,
            RawUnit::MicroTesla => RawUnitView::MicroTesla,
            RawUnit::StandardGravity => RawUnitView::StandardGravity,
            RawUnit::MetersPerSec2 => RawUnitView::MetersPerSec2,
        },
    }
}

/// 契約の失敗を画面の入力へ写す。OS 固有値は画面へ漏らさない（security.md A09）。
fn to_failure(f: RawSensorFailure) -> RawFailureView {
    match f {
        RawSensorFailure::UnsupportedOnOs => RawFailureView::UnsupportedOnOs,
        RawSensorFailure::NotPresent => RawFailureView::NotPresent,
        RawSensorFailure::PermissionDenied => RawFailureView::PermissionDenied,
        RawSensorFailure::Failed => RawFailureView::Failed,
    }
}

/// 種類ごとの記憶。
#[derive(Default)]
struct KindDriver {
    /// OS の購読を開始済みの世代。
    started_session: Option<u64>,
    /// 失敗を送信済みの世代（画面側が反映するまで再 `start` しない）。
    failed_session: Option<u64>,
    last_read: Option<Instant>,
}

/// driver の記憶（プロセス全体で 1 つ）。
#[derive(Default)]
struct DriverState {
    kinds: [KindDriver; 3],
}

static DRIVER: Mutex<Option<DriverState>> = Mutex::new(None);

fn send_failure(
    sender: &CompletionSender<MotionRawSensorsShowState>,
    kind: RawKindView,
    session: u64,
    f: RawFailureView,
) {
    let _ = sender.send(Box::new(move |s: &mut MotionRawSensorsShowState| {
        apply_failure(s, kind, session, f);
    }));
}

/// 全種類を順に駆動する。
fn drive(
    state: &MotionRawSensorsShowState,
    sender: &CompletionSender<MotionRawSensorsShowState>,
    sensors: &dyn MotionRawSensors,
    now: Instant,
    ds: &mut DriverState,
) {
    for (i, kind) in RawKind::ALL.into_iter().enumerate() {
        drive_kind(kind, state, sender, sensors, now, &mut ds.kinds[i]);
    }
}

/// 開始・停止・間隔判定・読取・送信省略を 1 種類について行う本体
/// （テストから時刻と実装を差し込めるよう切り出す）。
fn drive_kind(
    kind: RawKind,
    state: &MotionRawSensorsShowState,
    sender: &CompletionSender<MotionRawSensorsShowState>,
    sensors: &dyn MotionRawSensors,
    now: Instant,
    kd: &mut KindDriver,
) {
    let view_kind = to_kind_view(kind);
    let row = state.row(view_kind);
    if !row.wanted {
        // 立ち下がり: 購読を止める。待機では再描画を予約しない。
        if kd.started_session.take().is_some() {
            sensors.stop(kind);
        }
        kd.failed_session = None;
        kd.last_read = None;
        return;
    }
    let session = row.session;
    if kd.failed_session == Some(session) {
        // 失敗を送信済み。画面側が反映するまで何もしない。
        return;
    }
    if kd.started_session != Some(session) {
        // 立ち上がり（または世代交代）: 古い購読を先に止めてから開始する。
        if kd.started_session.take().is_some() {
            sensors.stop(kind);
        }
        if let Err(e) = sensors.start(kind) {
            sensors.stop(kind);
            kd.failed_session = Some(session);
            send_failure(sender, view_kind, session, to_failure(e));
            return;
        }
        kd.started_session = Some(session);
        kd.last_read = None;
    }

    if let Some(prev) = kd.last_read {
        let elapsed = now.saturating_duration_since(prev);
        if elapsed < READ_INTERVAL {
            sender.request_repaint_after(READ_INTERVAL - elapsed);
            return;
        }
    }
    kd.last_read = Some(now);
    // 送信を省略する分岐でも次回の読み取りを予約する。
    sender.request_repaint_after(READ_INTERVAL);

    // 非有限・種類違いの単位は値なしとして扱う（実装の取りこぼしに対する多重防御）。
    let next: Option<RawReadingView> = match sensors
        .read(kind)
        .map(|o| o.and_then(|r| validate_reading(kind, r)))
    {
        Ok(r) => r.map(to_view),
        Err(e) => {
            sensors.stop(kind);
            kd.started_session = None;
            kd.failed_session = Some(session);
            send_failure(sender, view_kind, session, to_failure(e));
            return;
        }
    };

    // 表示上の値が同じなら送らない。
    let shown = match (row.view, next) {
        (RawSensorView::Live(cur), Some(n)) => display_equal(&cur, &n),
        (RawSensorView::Waiting, None) => true,
        _ => false,
    };
    if shown {
        return;
    }
    let _ = sender.send(Box::new(move |s: &mut MotionRawSensorsShowState| {
        apply_reading(s, view_kind, session, next);
    }));
}

/// `registry.rs` の `effect` 区画から `motion_raw_sensors_show` 画面用に配線される effect ドライバ。
pub fn motion_raw_sensors_show_effect(
    state: &MotionRawSensorsShowState,
    sender: &CompletionSender<MotionRawSensorsShowState>,
) {
    let mut guard = DRIVER.lock().unwrap_or_else(|p| p.into_inner());
    let ds = guard.get_or_insert_with(DriverState::default);
    drive(state, sender, implementation().as_ref(), Instant::now(), ds);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::motion_raw_sensors_show::{row_text, start_all, stop_all};
    use fandhe_device::motion_raw_sensors::UnsupportedMotionRawSensors as Unsupported;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<MotionRawSensorsShowState>>;
    type ReadResult = Result<Option<RawReading>, RawSensorFailure>;

    fn channel() -> (CompletionSender<MotionRawSensorsShowState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply_all(rx: &Rx, s: &mut MotionRawSensorsShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    fn idx(k: RawKind) -> usize {
        RawKind::ALL.iter().position(|x| *x == k).unwrap()
    }

    fn rd(x: f64, unit: RawUnit) -> RawReading {
        RawReading {
            x,
            y: -0.003,
            z: 0.001,
            unit,
        }
    }

    /// 種類別に回数と結果を持つ偽実装。
    struct Fake {
        start: [Result<(), RawSensorFailure>; 3],
        read: [StdMutex<ReadResult>; 3],
        log: StdMutex<Vec<(&'static str, RawKind)>>,
    }

    impl Fake {
        fn new(read: ReadResult) -> Self {
            Self {
                start: [Ok(()); 3],
                read: [
                    StdMutex::new(read),
                    StdMutex::new(read),
                    StdMutex::new(read),
                ],
                log: StdMutex::new(Vec::new()),
            }
        }
        fn count(&self, op: &str, k: RawKind) -> usize {
            self.log
                .lock()
                .unwrap()
                .iter()
                .filter(|(o, kk)| *o == op && *kk == k)
                .count()
        }
        fn set_read(&self, k: RawKind, r: ReadResult) {
            *self.read[idx(k)].lock().unwrap() = r;
        }
    }

    impl MotionRawSensors for Fake {
        fn start(&self, kind: RawKind) -> Result<(), RawSensorFailure> {
            self.log.lock().unwrap().push(("start", kind));
            self.start[idx(kind)]
        }
        fn read(&self, kind: RawKind) -> ReadResult {
            self.log.lock().unwrap().push(("read", kind));
            *self.read[idx(kind)].lock().unwrap()
        }
        fn stop(&self, kind: RawKind) {
            self.log.lock().unwrap().push(("stop", kind));
        }
    }

    fn started() -> MotionRawSensorsShowState {
        let mut s = MotionRawSensorsShowState::default();
        start_all(&mut s);
        s
    }

    use RawKind::{AngularVelocity as Gyro, LinearAcceleration as Lin, MagneticField as Mag};

    #[test]
    fn live_values_per_kind() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        fake.set_read(Gyro, Ok(Some(rd(0.012, RawUnit::RadPerSec))));
        fake.set_read(Mag, Ok(Some(rd(40.0, RawUnit::MicroTesla))));
        fake.set_read(Lin, Ok(Some(rd(0.5, RawUnit::MetersPerSec2))));
        let mut s = started();
        drive(&s, &tx, &fake, Instant::now(), &mut DriverState::default());
        apply_all(&rx, &mut s);
        assert_eq!(
            row_text(&s, RawKindView::AngularVelocity),
            "角速度: x 0.012 / y -0.003 / z 0.001 rad/s"
        );
        assert!(row_text(&s, RawKindView::MagneticField).ends_with("µT"));
        assert!(row_text(&s, RawKindView::LinearAcceleration).ends_with("m/s²"));
    }

    #[test]
    fn start_failure_stops_then_fails_and_others_continue() {
        let (tx, rx) = channel();
        let mut fake = Fake::new(Ok(Some(rd(1.0, RawUnit::MicroTesla))));
        fake.start[idx(Gyro)] = Err(RawSensorFailure::NotPresent);
        let mut s = started();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        // 反映前にもう一度フレームが来ても再 start しない。
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.count("start", Gyro), 1);
        assert_eq!(fake.count("stop", Gyro), 1);
        assert_eq!(
            s.angular_velocity.view,
            RawSensorView::Failed(RawFailureView::NotPresent)
        );
        assert!(s.magnetic_field.wanted);
        assert_eq!(fake.count("stop", Mag), 0);
        let log = fake.log.lock().unwrap();
        let first_stop = log.iter().position(|e| *e == ("stop", Gyro)).unwrap();
        let first_start = log.iter().position(|e| *e == ("start", Gyro)).unwrap();
        assert!(first_start < first_stop);
    }

    #[test]
    fn read_permission_denied_stops_then_fails() {
        let (tx, rx) = channel();
        let fake = Fake::new(Err(RawSensorFailure::PermissionDenied));
        let mut s = started();
        drive(&s, &tx, &fake, Instant::now(), &mut DriverState::default());
        apply_all(&rx, &mut s);
        assert_eq!(fake.count("stop", Gyro), 1);
        assert_eq!(
            s.angular_velocity.view,
            RawSensorView::Failed(RawFailureView::PermissionDenied)
        );
    }

    #[test]
    fn nan_and_mismatched_unit_never_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        fake.set_read(Gyro, Ok(Some(rd(f64::NAN, RawUnit::RadPerSec))));
        fake.set_read(Mag, Ok(Some(rd(1.0, RawUnit::RadPerSec))));
        fake.set_read(Lin, Ok(Some(rd(f64::INFINITY, RawUnit::StandardGravity))));
        let mut s = started();
        drive(&s, &tx, &fake, Instant::now(), &mut DriverState::default());
        apply_all(&rx, &mut s);
        for v in [
            s.angular_velocity.view,
            s.magnetic_field.view,
            s.linear_acceleration.view,
        ] {
            assert_eq!(v, RawSensorView::Waiting);
        }
    }

    #[test]
    fn stale_session_completion_is_not_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(Some(rd(1.0, RawUnit::MicroTesla))));
        fake.set_read(Gyro, Ok(Some(rd(1.0, RawUnit::RadPerSec))));
        fake.set_read(Lin, Ok(Some(rd(1.0, RawUnit::MetersPerSec2))));
        let mut s = started();
        drive(&s, &tx, &fake, Instant::now(), &mut DriverState::default());
        // 完了が届く前に停止して再開始（世代 2）。
        stop_all(&mut s);
        start_all(&mut s);
        apply_all(&rx, &mut s);
        assert_eq!(s.angular_velocity.view, RawSensorView::Waiting);
        assert_eq!(s.magnetic_field.view, RawSensorView::Waiting);
    }

    #[test]
    fn falling_edge_stops_once_and_never_reads_again() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = started();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        stop_all(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, &mut ds);
        for k in RawKind::ALL {
            assert_eq!(fake.count("start", k), 1);
            assert_eq!(fake.count("stop", k), 1);
            assert_eq!(fake.count("read", k), 1);
        }
    }

    #[test]
    fn idle_never_reads_or_starts() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        drive(
            &MotionRawSensorsShowState::default(),
            &tx,
            &fake,
            Instant::now(),
            &mut DriverState::default(),
        );
        assert!(fake.log.lock().unwrap().is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn skips_read_within_interval_and_same_value_is_not_sent() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        fake.set_read(Gyro, Ok(Some(rd(0.0121, RawUnit::RadPerSec))));
        let mut s = started();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(50), &mut ds);
        assert_eq!(fake.count("read", Gyro), 1);
        fake.set_read(Gyro, Ok(Some(rd(0.0124, RawUnit::RadPerSec))));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.count("read", Gyro), 2);
    }

    #[test]
    fn new_session_restarts_after_stop_in_between() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = started();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        stop_all(&mut s);
        start_all(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
        assert_eq!(fake.count("start", Gyro), 2);
        assert_eq!(fake.count("stop", Gyro), 1);
    }

    #[test]
    fn unsupported_impl_never_becomes_live() {
        let (tx, rx) = channel();
        let mut s = started();
        drive(
            &s,
            &tx,
            &Unsupported,
            Instant::now(),
            &mut DriverState::default(),
        );
        apply_all(&rx, &mut s);
        for v in [
            s.angular_velocity.view,
            s.magnetic_field.view,
            s.linear_acceleration.view,
        ] {
            assert_eq!(v, RawSensorView::Failed(RawFailureView::UnsupportedOnOs));
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
    #[test]
    fn host_fallback_never_succeeds() {
        let imp = implementation();
        for k in RawKind::ALL {
            assert_eq!(imp.start(k), Err(RawSensorFailure::UnsupportedOnOs));
            assert_eq!(imp.read(k), Err(RawSensorFailure::UnsupportedOnOs));
        }
    }
}
