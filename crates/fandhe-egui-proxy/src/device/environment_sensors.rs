//! Phase 11 の拡充ループ（親 #123、Issue #609）「気圧・照度・近接」の effect ドライバ。
//! `fandhe_core::screens::environment_sensors_show`（端末依存ゼロ）の種類ごとの `wanted` の立ち上がりで
//! `fandhe_device::environment_sensors::EnvironmentSensors::start`、立ち下がりで `stop` を呼び、購読中は
//! [`READ_INTERVAL`] ごとに `read` して結果を注入する（`registry.rs` の `effect` 区画から
//! 毎フレーム同期に呼ばれる）。`compass.rs` と同型の間隔つき pull を種類ごとに回す。
//!
//! **間隔つき pull**: 最終読取から `READ_INTERVAL` 未満なら OS を読まず残り時間で再描画を予約する。
//! 読んだら送信を省く分岐でも次回を必ず予約する（入力が無いと egui が描画せずポーリングが止まる、
//! 契約表 4）。待機（購読なし）では予約しない。
//!
//! 失敗（開始失敗・読み取り失敗・期限切れ）を確定する前に必ず `stop(kind)` する（契約表 3・9）。
//! 期限は単調時計（`Instant`）由来のミリ秒だけで測る。完了は読み取り時の `session` を持たせ、
//! 古い世代の完了は画面側で捨てられる。1 つの種類の失敗で他の種類を止めない。
//!
//! 判定と状態更新は `DRIVER` のロックの内側で行い、経路はこのドライバ 1 本に集約する（契約表 2・11）。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS・Android・Windows は
//! `ios_environment_sensors.rs` 等のスタブ（後続 #610・#611・#612 が実体を入れる）、
//! macOS・Linux は `UnsupportedEnvironmentSensors` 固定。Web は対応表で `no` のため配線しない。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fandhe_core::screens::environment_sensors_show::{
    apply_failure, apply_reading, display_equal, EnvironmentSensorsShowState, ProximityView,
    ReadingView, SensorFailureView, SensorKindView, SensorView,
};
#[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
use fandhe_device::environment_sensors::UnsupportedEnvironmentSensors;
use fandhe_device::environment_sensors::{
    validate_reading, EnvReading, EnvSensorFailure, EnvironmentSensors, Proximity, SensorKind,
    NO_READING_TIMEOUT_MS, READ_INTERVAL,
};

use crate::app::CompletionSender;
use fandhe_device::compass::WaitTracker;

/// 現在のプラットフォームに応じた実装を返す（プロセスで 1 個。購読を二重に作らないため
/// `OnceLock` で保持する）。
fn implementation() -> Arc<dyn EnvironmentSensors> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn EnvironmentSensors>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                super::windows_environment_sensors::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_environment_sensors::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_environment_sensors::implementation()
            }
            #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
            {
                Arc::new(UnsupportedEnvironmentSensors)
            }
        })
        .clone()
}

fn to_kind(k: SensorKind) -> SensorKindView {
    match k {
        SensorKind::Pressure => SensorKindView::Pressure,
        SensorKind::Light => SensorKindView::Light,
        SensorKind::Proximity => SensorKindView::Proximity,
    }
}

fn to_view(r: EnvReading) -> ReadingView {
    match r {
        EnvReading::PressureHpa(v) => ReadingView::PressureHpa(v),
        EnvReading::IlluminanceLux(v) => ReadingView::IlluminanceLux(v),
        EnvReading::Proximity(Proximity::Near) => ReadingView::Proximity(ProximityView::Near),
        EnvReading::Proximity(Proximity::Far) => ReadingView::Proximity(ProximityView::Far),
    }
}

/// 契約の失敗を画面の入力へ写す。OS 固有値は画面へ漏らさない（security.md A09）。
fn to_failure(f: EnvSensorFailure) -> SensorFailureView {
    match f {
        EnvSensorFailure::UnsupportedOnOs => SensorFailureView::UnsupportedOnOs,
        EnvSensorFailure::NotPresent => SensorFailureView::NotPresent,
        EnvSensorFailure::PermissionDenied => SensorFailureView::PermissionDenied,
        EnvSensorFailure::Failed => SensorFailureView::Failed,
    }
}

/// 種類ごとの記憶。
#[derive(Default)]
struct KindDriver {
    /// OS の購読を開始済みの世代。
    started_session: Option<u64>,
    /// 失敗を送信済みの世代（画面側が反映するまで再 `start` しない）。
    failed_session: Option<u64>,
    /// 購読を始めた時刻（経過ミリ秒の基準）。
    epoch: Option<Instant>,
    last_read: Option<Instant>,
    /// 反映済みの復帰回数。
    resume_seen: u64,
    tracker: WaitTracker,
}

/// driver の記憶（プロセス全体で 1 つ）。
#[derive(Default)]
struct DriverState {
    kinds: [KindDriver; 3],
    /// 背景中は読み取りも待機の計測も止める（復帰で起点を引き直す）。
    background: bool,
}

static DRIVER: Mutex<Option<DriverState>> = Mutex::new(None);

/// 背景からの復帰回数。プラットフォームが復帰イベントで増やし、ドライバは増えたときだけ
/// 待機の起点を引き直す（描画間隔からは推測しない。値なしが続けば必ず期限で `NoReading` になる）。
static RESUME_EPOCH: AtomicU64 = AtomicU64::new(0);

/// 背景中か。背景中は `NoReading` の待機を進めない。
static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// 背景遷移を記録する。各 OS の実装（iOS #610 の `DidEnterBackground`、Android #611 の `onPause`、
/// Windows #612 の最小化）が自分のモジュールから呼ぶ。(a) の時点では呼ぶ側が無い。
#[cfg(any(target_os = "ios", target_os = "android", target_os = "windows", test))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn note_backgrounded() {
    BACKGROUND.store(true, Ordering::SeqCst);
}

/// 背景からの復帰を記録する。各 OS の実装が前景復帰（iOS の復帰通知・Android の `onResume`・Windows の復元）から呼ぶ。
/// 復帰回数を先に増やしてから背景フラグを下ろす（フラグが下りた時点で起点の引き直しが見える）。
#[cfg(any(target_os = "ios", target_os = "android", target_os = "windows", test))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn note_resumed() {
    RESUME_EPOCH.fetch_add(1, Ordering::SeqCst);
    BACKGROUND.store(false, Ordering::SeqCst);
}

fn send_failure(
    sender: &CompletionSender<EnvironmentSensorsShowState>,
    kind: SensorKindView,
    session: u64,
    f: SensorFailureView,
) {
    let _ = sender.send(Box::new(move |s: &mut EnvironmentSensorsShowState| {
        apply_failure(s, kind, session, f);
    }));
}

/// 全種類を順に駆動する。
fn drive(
    state: &EnvironmentSensorsShowState,
    sender: &CompletionSender<EnvironmentSensorsShowState>,
    sensors: &dyn EnvironmentSensors,
    now: Instant,
    resume_epoch: u64,
    ds: &mut DriverState,
) {
    let background = ds.background;
    for (i, kind) in SensorKind::ALL.into_iter().enumerate() {
        drive_kind(
            kind,
            state,
            sender,
            sensors,
            now,
            resume_epoch,
            background,
            &mut ds.kinds[i],
        );
    }
}

/// 開始・停止・間隔判定・読取・送信省略を 1 種類について行う本体
/// （テストから時刻と実装を差し込めるよう切り出す）。
#[allow(clippy::too_many_arguments)]
fn drive_kind(
    kind: SensorKind,
    state: &EnvironmentSensorsShowState,
    sender: &CompletionSender<EnvironmentSensorsShowState>,
    sensors: &dyn EnvironmentSensors,
    now: Instant,
    resume_epoch: u64,
    background: bool,
    kd: &mut KindDriver,
) {
    let view_kind = to_kind(kind);
    let row = state.row(view_kind);
    if !row.wanted {
        // 立ち下がり: 購読を止める。待機では再描画を予約しない。
        if kd.started_session.take().is_some() {
            sensors.stop(kind);
        }
        kd.failed_session = None;
        kd.epoch = None;
        kd.last_read = None;
        kd.tracker.reset();
        return;
    }
    let session = row.session;
    if kd.failed_session == Some(session) {
        // 失敗を送信済み。画面側が反映するまで何もしない。
        return;
    }
    if background {
        // 背景中は待機を進めない。復帰後の最初の読み取りで起点を引き直す。
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
        kd.epoch = Some(now);
        kd.last_read = None;
        kd.tracker.begin(0.0);
        kd.resume_seen = resume_epoch;
    }

    // 背景遷移の間は待機に数えない（復帰イベントがあったときだけ起点を引き直す）。
    // 復帰の記録は起点を引き直す読み取りまで消費しない（間隔待ちの early return で失わない）。
    let resumed = kd.resume_seen != resume_epoch;
    if let Some(prev) = kd.last_read {
        let elapsed = now.saturating_duration_since(prev);
        if elapsed < READ_INTERVAL {
            sender.request_repaint_after(READ_INTERVAL - elapsed);
            return;
        }
    }
    kd.last_read = Some(now);
    kd.resume_seen = resume_epoch;
    // 送信を省略する分岐でも次回の読み取りを予約する。
    sender.request_repaint_after(READ_INTERVAL);

    let now_ms = kd.epoch.map_or(0.0, |e| {
        now.saturating_duration_since(e).as_secs_f64() * 1000.0
    });
    if resumed {
        kd.tracker.begin(now_ms);
    }
    // 範囲外・種類違いは値なしとして扱う（実装の取りこぼしに対する多重防御）。
    let read = sensors
        .read(kind)
        .map(|o| o.and_then(|r| validate_reading(kind, r)));
    let next: Option<ReadingView> = match read {
        Ok(Some(r)) => {
            kd.tracker
                .observe_within(true, now_ms, NO_READING_TIMEOUT_MS);
            Some(to_view(r))
        }
        Ok(None) => {
            if kd
                .tracker
                .observe_within(false, now_ms, NO_READING_TIMEOUT_MS)
            {
                sensors.stop(kind);
                kd.started_session = None;
                kd.failed_session = Some(session);
                send_failure(sender, view_kind, session, SensorFailureView::NoReading);
                return;
            }
            None
        }
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
        (SensorView::Live(cur), Some(n)) => display_equal(&cur, &n),
        (SensorView::Waiting, None) => true,
        _ => false,
    };
    if shown {
        return;
    }
    let _ = sender.send(Box::new(move |s: &mut EnvironmentSensorsShowState| {
        apply_reading(s, view_kind, session, next);
    }));
}

/// `registry.rs` の `effect` 区画から `environment_sensors_show` 画面用に配線される effect ドライバ。
pub fn environment_sensors_show_effect(
    state: &EnvironmentSensorsShowState,
    sender: &CompletionSender<EnvironmentSensorsShowState>,
) {
    let mut guard = DRIVER.lock().unwrap_or_else(|p| p.into_inner());
    let ds = guard.get_or_insert_with(DriverState::default);
    ds.background = BACKGROUND.load(Ordering::SeqCst);
    drive(
        state,
        sender,
        implementation().as_ref(),
        Instant::now(),
        RESUME_EPOCH.load(Ordering::SeqCst),
        ds,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::environment_sensors_show::{
        row_text, start_ambient, start_proximity, stop_ambient,
    };
    use fandhe_device::environment_sensors::UnsupportedEnvironmentSensors as Unsupported;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<EnvironmentSensorsShowState>>;
    type ReadResult = Result<Option<EnvReading>, EnvSensorFailure>;

    fn channel() -> (CompletionSender<EnvironmentSensorsShowState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply_all(rx: &Rx, s: &mut EnvironmentSensorsShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    fn idx(k: SensorKind) -> usize {
        SensorKind::ALL.iter().position(|x| *x == k).unwrap()
    }

    /// 種類別に回数と結果を持つ偽実装。
    struct Fake {
        start: [Result<(), EnvSensorFailure>; 3],
        read: [StdMutex<ReadResult>; 3],
        /// 呼び出し順（"start:Pressure" 等）。
        log: StdMutex<Vec<(&'static str, SensorKind)>>,
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
        fn count(&self, op: &str, k: SensorKind) -> usize {
            self.log
                .lock()
                .unwrap()
                .iter()
                .filter(|(o, kk)| *o == op && *kk == k)
                .count()
        }
        fn set_read(&self, k: SensorKind, r: ReadResult) {
            *self.read[idx(k)].lock().unwrap() = r;
        }
    }

    impl EnvironmentSensors for Fake {
        fn start(&self, kind: SensorKind) -> Result<(), EnvSensorFailure> {
            self.log.lock().unwrap().push(("start", kind));
            self.start[idx(kind)]
        }
        fn read(&self, kind: SensorKind) -> ReadResult {
            self.log.lock().unwrap().push(("read", kind));
            *self.read[idx(kind)].lock().unwrap()
        }
        fn stop(&self, kind: SensorKind) {
            self.log.lock().unwrap().push(("stop", kind));
        }
    }

    fn ambient() -> EnvironmentSensorsShowState {
        let mut s = EnvironmentSensorsShowState::default();
        start_ambient(&mut s);
        s
    }

    use SensorKind::{Light, Pressure, Proximity as Prox};

    #[test]
    fn live_values_per_kind() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        fake.set_read(Pressure, Ok(Some(EnvReading::PressureHpa(1013.24))));
        fake.set_read(Light, Ok(Some(EnvReading::IlluminanceLux(350.0))));
        let mut s = ambient();
        let mut ds = DriverState::default();
        drive(&s, &tx, &fake, Instant::now(), 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(row_text(&s, SensorKindView::Pressure), "気圧: 1013.2 hPa");
        assert_eq!(row_text(&s, SensorKindView::Light), "照度: 350 lx");
        // 近接は未購読のため触らない。
        assert_eq!(fake.count("start", Prox), 0);
        assert_eq!(fake.count("read", Prox), 0);
    }

    #[test]
    fn start_failure_stops_then_fails_and_others_continue() {
        let (tx, rx) = channel();
        let mut fake = Fake::new(Ok(Some(EnvReading::IlluminanceLux(1.0))));
        fake.start[idx(Pressure)] = Err(EnvSensorFailure::NotPresent);
        let mut s = ambient();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        // 反映前にもう一度フレームが来ても再 start しない。
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.count("start", Pressure), 1);
        assert_eq!(fake.count("stop", Pressure), 1);
        assert_eq!(
            s.pressure.view,
            SensorView::Failed(SensorFailureView::NotPresent)
        );
        // 照度は購読を続ける。
        assert!(s.light.wanted);
        assert_eq!(fake.count("stop", Light), 0);
        // 失敗は stop → 通知の順（start の後に stop が来る）。
        let log = fake.log.lock().unwrap();
        let first_stop = log.iter().position(|e| *e == ("stop", Pressure)).unwrap();
        let first_start = log.iter().position(|e| *e == ("start", Pressure)).unwrap();
        assert!(first_start < first_stop);
    }

    #[test]
    fn read_permission_denied_stops_then_fails() {
        let (tx, rx) = channel();
        let fake = Fake::new(Err(EnvSensorFailure::PermissionDenied));
        let mut s = EnvironmentSensorsShowState::default();
        start_proximity(&mut s);
        drive(
            &s,
            &tx,
            &fake,
            Instant::now(),
            0,
            &mut DriverState::default(),
        );
        apply_all(&rx, &mut s);
        assert_eq!(fake.count("stop", Prox), 1);
        assert_eq!(
            s.proximity.view,
            SensorView::Failed(SensorFailureView::PermissionDenied)
        );
    }

    #[test]
    fn no_value_for_timeout_stops_then_no_reading() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = ambient();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        let at = |ms: f64| t0 + Duration::from_secs_f64(ms / 1000.0);
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        let mut ms = 500.0;
        while ms <= NO_READING_TIMEOUT_MS {
            drive(&s, &tx, &fake, at(ms), 0, &mut ds);
            ms += 500.0;
        }
        apply_all(&rx, &mut s);
        assert_eq!(s.pressure.view, SensorView::Waiting);
        assert_eq!(fake.count("stop", Pressure), 0);
        drive(
            &s,
            &tx,
            &fake,
            at(NO_READING_TIMEOUT_MS + 200.0),
            0,
            &mut ds,
        );
        apply_all(&rx, &mut s);
        assert_eq!(fake.count("stop", Pressure), 1);
        assert_eq!(
            s.pressure.view,
            SensorView::Failed(SensorFailureView::NoReading)
        );
    }

    #[test]
    fn out_of_range_and_mismatched_values_never_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        fake.set_read(Pressure, Ok(Some(EnvReading::PressureHpa(5.0))));
        fake.set_read(Light, Ok(Some(EnvReading::PressureHpa(1000.0))));
        let mut s = ambient();
        drive(
            &s,
            &tx,
            &fake,
            Instant::now(),
            0,
            &mut DriverState::default(),
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.pressure.view, SensorView::Waiting);
        assert_eq!(s.light.view, SensorView::Waiting);
    }

    #[test]
    fn stale_session_completion_is_not_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(Some(EnvReading::PressureHpa(1000.0))));
        let mut s = EnvironmentSensorsShowState::default();
        start_ambient(&mut s);
        drive(
            &s,
            &tx,
            &fake,
            Instant::now(),
            0,
            &mut DriverState::default(),
        );
        // 完了が届く前に停止して再開始（世代 2）。
        stop_ambient(&mut s);
        start_ambient(&mut s);
        apply_all(&rx, &mut s);
        assert_eq!(s.pressure.view, SensorView::Waiting);
        assert_eq!(s.light.view, SensorView::Waiting);
    }

    #[test]
    fn falling_edge_stops_once_and_does_not_repaint() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = ambient();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        stop_ambient(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, 0, &mut ds);
        assert_eq!(fake.count("stop", Pressure), 1);
        assert_eq!(fake.count("stop", Light), 1);
        assert_eq!(fake.count("read", Pressure), 1);
    }

    #[test]
    fn idle_never_reads_or_starts() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        drive(
            &EnvironmentSensorsShowState::default(),
            &tx,
            &fake,
            Instant::now(),
            0,
            &mut DriverState::default(),
        );
        assert!(fake.log.lock().unwrap().is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn skips_read_within_interval_and_same_value_is_not_sent() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        fake.set_read(Pressure, Ok(Some(EnvReading::PressureHpa(1013.21))));
        let mut s = ambient();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(50), 0, &mut ds);
        assert_eq!(fake.count("read", Pressure), 1);
        // 表示上同じなら送らない（照度は値なしのまま Waiting で同じ）。
        fake.set_read(Pressure, Ok(Some(EnvReading::PressureHpa(1013.24))));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.count("read", Pressure), 2);
    }

    #[test]
    fn long_gap_without_resume_still_times_out() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = ambient();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(
            s.pressure.view,
            SensorView::Failed(SensorFailureView::NoReading)
        );
    }

    #[test]
    fn background_pauses_wait_and_resume_restarts_origin() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = ambient();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        note_backgrounded();
        ds.background = BACKGROUND.load(Ordering::SeqCst);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), 0, &mut ds);
        note_resumed();
        ds.background = BACKGROUND.load(Ordering::SeqCst);
        let epoch = RESUME_EPOCH.load(Ordering::SeqCst);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), epoch, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.count("stop", Pressure), 0);
        assert_eq!(s.pressure.view, SensorView::Waiting);
    }

    #[test]
    fn new_session_restarts_after_stop_in_between() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = ambient();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        stop_ambient(&mut s);
        start_ambient(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        // 古い購読を止めてから開始し直す。
        assert_eq!(fake.count("start", Pressure), 2);
        assert_eq!(fake.count("stop", Pressure), 1);
    }

    #[test]
    fn unsupported_impl_never_becomes_live() {
        let (tx, rx) = channel();
        let mut s = ambient();
        start_proximity(&mut s);
        drive(
            &s,
            &tx,
            &Unsupported,
            Instant::now(),
            0,
            &mut DriverState::default(),
        );
        apply_all(&rx, &mut s);
        for v in [s.pressure.view, s.light.view, s.proximity.view] {
            assert_eq!(v, SensorView::Failed(SensorFailureView::UnsupportedOnOs));
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
    #[test]
    fn host_fallback_never_succeeds() {
        let imp = implementation();
        for k in SensorKind::ALL {
            assert_eq!(imp.start(k), Err(EnvSensorFailure::UnsupportedOnOs));
            assert_eq!(imp.read(k), Err(EnvSensorFailure::UnsupportedOnOs));
        }
    }
}
