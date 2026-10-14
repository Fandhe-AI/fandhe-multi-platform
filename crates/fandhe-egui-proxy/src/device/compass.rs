//! Phase 11 の拡充ループ（親 #1154、Issue #1404）「方位（コンパス）」の effect ドライバ。
//! `fandhe_core::screens::compass_show`（端末依存ゼロ）の `wanted` の立ち上がりで
//! `fandhe_device::compass::Compass::start`、立ち下がりで `stop` を呼び、購読中は
//! [`READ_INTERVAL`] ごとに `read` して結果を注入する（`registry.rs` の `effect` 区画から
//! 毎フレーム同期に呼ばれる）。
//!
//! **間隔つき pull**（`network_status.rs` と同型）: 最終読取から `READ_INTERVAL` 未満なら OS を
//! 読まず残り時間で再描画を予約する。読んだら送信を省く分岐でも次回を必ず予約する
//! （入力が無いと egui が描画せずポーリングが止まる、契約表 4）。待機（購読なし）では予約しない。
//!
//! 失敗（開始失敗・読み取り失敗・期限切れ）を確定する前に必ず `stop()` する（契約表 3・9）。
//! 期限は単調時計（`Instant`）由来のミリ秒だけで測る。完了は読み取り時の `session` を持たせ、
//! 古い世代の完了は画面側で捨てられる。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。Windows は `windows_compass.rs`、iOS は `ios_compass.rs`、Android は
//! 後続の子（#1404 の (b)）が埋めるまでスタブ（`UnsupportedCompass`）、Web は `fandhe-web-dom` 側、
//! macOS・Linux は非対応。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fandhe_core::screens::compass_show::{
    apply_failure, apply_reading, AccuracyView, CompassFailureView, CompassShowState, CompassView,
    HeadingView,
};
#[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
use fandhe_device::compass::UnsupportedCompass;
use fandhe_device::compass::{
    Accuracy, Compass, CompassFailure, Heading, WaitTracker, READ_INTERVAL,
};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す（プロセスで 1 個。購読を二重に作らないため
/// `OnceLock` で保持する）。
fn implementation() -> Arc<dyn Compass> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn Compass>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                super::windows_compass::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_compass::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_compass::implementation()
            }
            #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
            {
                Arc::new(UnsupportedCompass)
            }
        })
        .clone()
}

fn to_view(h: Heading) -> HeadingView {
    HeadingView {
        degrees: h.degrees,
        accuracy: match h.accuracy {
            Accuracy::Unknown => AccuracyView::Unknown,
            Accuracy::Unreliable => AccuracyView::Unreliable,
            Accuracy::Approximate => AccuracyView::Approximate,
            Accuracy::High => AccuracyView::High,
        },
    }
}

/// 契約の失敗を画面の入力へ写す。OS 固有値は画面へ漏らさない（security.md A09）。
fn to_failure(f: CompassFailure) -> CompassFailureView {
    match f {
        CompassFailure::Unsupported => CompassFailureView::Unsupported,
        CompassFailure::PermissionDenied => CompassFailureView::PermissionDenied,
        CompassFailure::Failed => CompassFailureView::Failed,
    }
}

/// driver の記憶（プロセス全体で 1 つ）。
#[derive(Default)]
struct DriverState {
    /// OS の購読を開始済みの世代。
    started_session: Option<u64>,
    /// 失敗を送信済みの世代（画面側が反映するまで再 `start` しない）。
    failed_session: Option<u64>,
    /// 購読を始めた時刻（経過ミリ秒の基準）。
    epoch: Option<Instant>,
    last_read: Option<Instant>,
    /// 反映済みの復帰回数。
    resume_seen: u64,
    /// 背景中は読み取りも待機の計測も止める（復帰で起点を引き直す）。
    background: bool,
    tracker: WaitTracker,
}

static DRIVER: Mutex<Option<DriverState>> = Mutex::new(None);

fn send_failure(sender: &CompletionSender<CompassShowState>, session: u64, f: CompassFailureView) {
    let _ = sender.send(Box::new(move |s: &mut CompassShowState| {
        apply_failure(s, session, f);
    }));
}

/// 背景からの復帰回数。プラットフォームが復帰イベントで増やし、ドライバは増えたときだけ
/// 待機の起点を引き直す（描画間隔からは推測しない。値なしが続けば必ず期限で `NoReading` になる）。
static RESUME_EPOCH: AtomicU64 = AtomicU64::new(0);

/// 背景中か。背景中は `NoReading` の待機を進めない。
static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// 背景遷移を記録する（iOS の `DidEnterBackground` から呼ばれる）。
#[cfg(any(target_os = "ios", test))]
pub(crate) fn note_backgrounded() {
    BACKGROUND.store(true, Ordering::SeqCst);
}

/// 背景からの復帰を記録する（iOS の前景復帰通知から呼ばれる）。
/// 復帰回数を先に増やしてから背景フラグを下ろす（フラグが下りた時点で起点の引き直しが見える）。
#[cfg(any(target_os = "ios", test))]
pub(crate) fn note_resumed() {
    RESUME_EPOCH.fetch_add(1, Ordering::SeqCst);
    BACKGROUND.store(false, Ordering::SeqCst);
}

/// 開始・停止・間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &CompassShowState,
    sender: &CompletionSender<CompassShowState>,
    compass: &dyn Compass,
    now: Instant,
    resume_epoch: u64,
    ds: &mut DriverState,
) {
    if !state.wanted {
        // 立ち下がり: 購読を止める。待機では再描画を予約しない。
        if ds.started_session.take().is_some() {
            compass.stop();
        }
        ds.failed_session = None;
        ds.epoch = None;
        ds.last_read = None;
        ds.tracker.reset();
        return;
    }
    let session = state.session;
    if ds.failed_session == Some(session) {
        // 失敗を送信済み。画面側が反映するまで何もしない。
        return;
    }
    if ds.background {
        // 背景中は待機を進めない。復帰後の最初の読み取りで起点を引き直す。
        return;
    }
    if ds.started_session != Some(session) {
        // 立ち上がり（または世代交代）: 古い購読を先に止めてから開始する。
        if ds.started_session.take().is_some() {
            compass.stop();
        }
        if let Err(e) = compass.start() {
            compass.stop();
            ds.failed_session = Some(session);
            send_failure(sender, session, to_failure(e));
            return;
        }
        ds.started_session = Some(session);
        ds.epoch = Some(now);
        ds.last_read = None;
        ds.tracker.begin(0.0);
        ds.resume_seen = resume_epoch;
    }

    // 背景遷移の間は待機に数えない（復帰イベントがあったときだけ起点を引き直す）。
    // 復帰の記録は起点を引き直す読み取りまで消費しない（間隔待ちの early return で失わない）。
    let resumed = ds.resume_seen != resume_epoch;
    if let Some(prev) = ds.last_read {
        let elapsed = now.saturating_duration_since(prev);
        if elapsed < READ_INTERVAL {
            sender.request_repaint_after(READ_INTERVAL - elapsed);
            return;
        }
    }
    ds.last_read = Some(now);
    ds.resume_seen = resume_epoch;
    // 送信を省略する分岐でも次回の読み取りを予約する。
    sender.request_repaint_after(READ_INTERVAL);

    let now_ms = ds.epoch.map_or(0.0, |e| {
        now.saturating_duration_since(e).as_secs_f64() * 1000.0
    });
    if resumed {
        ds.tracker.begin(now_ms);
    }
    let next: Option<HeadingView> = match compass.read() {
        Ok(Some(h)) => {
            ds.tracker.observe(true, now_ms);
            Some(to_view(h))
        }
        Ok(None) => {
            if ds.tracker.observe(false, now_ms) {
                compass.stop();
                ds.started_session = None;
                ds.failed_session = Some(session);
                send_failure(sender, session, CompassFailureView::NoReading);
                return;
            }
            None
        }
        Err(e) => {
            compass.stop();
            ds.started_session = None;
            ds.failed_session = Some(session);
            send_failure(sender, session, to_failure(e));
            return;
        }
    };

    // 表示上の値（整数度・方位・精度）が同じなら送らない。
    let shown = match (state.view, next) {
        (CompassView::Live(cur), Some(n)) => {
            use fandhe_core::screens::compass_show::{display_degrees, point_label};
            display_degrees(cur.degrees) == display_degrees(n.degrees)
                && point_label(cur.degrees) == point_label(n.degrees)
                && cur.accuracy == n.accuracy
        }
        (CompassView::Waiting, None) => true,
        _ => false,
    };
    if shown {
        return;
    }
    let _ = sender.send(Box::new(move |s: &mut CompassShowState| {
        apply_reading(s, session, next);
    }));
}

/// `registry.rs` の `effect` 区画から `compass_show` 画面用に配線される effect ドライバ。
pub fn compass_show_effect(state: &CompassShowState, sender: &CompletionSender<CompassShowState>) {
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
    use fandhe_core::screens::compass_show::{heading_text, start, stop};
    use fandhe_device::compass::{UnsupportedCompass as Unsupported, NO_READING_TIMEOUT_MS};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<CompassShowState>>;

    fn channel() -> (CompletionSender<CompassShowState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply_all(rx: &Rx, s: &mut CompassShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    struct Fake {
        start: Result<(), CompassFailure>,
        read: StdMutex<Result<Option<Heading>, CompassFailure>>,
        starts: AtomicUsize,
        reads: AtomicUsize,
        stops: AtomicUsize,
    }

    impl Fake {
        fn new(read: Result<Option<Heading>, CompassFailure>) -> Self {
            Self {
                start: Ok(()),
                read: StdMutex::new(read),
                starts: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
            }
        }
        fn counts(&self) -> (usize, usize, usize) {
            (
                self.starts.load(Ordering::SeqCst),
                self.reads.load(Ordering::SeqCst),
                self.stops.load(Ordering::SeqCst),
            )
        }
    }

    impl Compass for Fake {
        fn start(&self) -> Result<(), CompassFailure> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.start
        }
        fn read(&self) -> Result<Option<Heading>, CompassFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            *self.read.lock().unwrap()
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn heading(d: f64) -> Heading {
        Heading {
            degrees: d,
            accuracy: Accuracy::High,
        }
    }

    fn wanted() -> CompassShowState {
        let mut s = CompassShowState::default();
        start(&mut s);
        s
    }

    #[test]
    fn same_degree_but_new_point_is_sent() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(Some(heading(22.4))));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(heading_text(&s), "方位 22°（北）");
        *fake.read.lock().unwrap() = Ok(Some(heading(22.6)));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(heading_text(&s), "方位 23°（北東）");
    }

    #[test]
    fn start_once_stop_once_and_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(Some(heading(90.0))));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        assert_eq!(fake.counts(), (1, 2, 0));
        assert_eq!(heading_text(&s), "方位 90°（東）");
        stop(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 3, 0, &mut ds);
        assert_eq!(fake.counts(), (1, 2, 1));
    }

    #[test]
    fn idle_never_reads_or_starts() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(Some(heading(1.0))));
        drive(
            &CompassShowState::default(),
            &tx,
            &fake,
            Instant::now(),
            0,
            &mut DriverState::default(),
        );
        assert_eq!(fake.counts(), (0, 0, 0));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn start_failures_never_become_live_and_do_not_retry() {
        for f in [
            CompassFailure::Unsupported,
            CompassFailure::PermissionDenied,
            CompassFailure::Failed,
        ] {
            let (tx, rx) = channel();
            let mut fake = Fake::new(Ok(Some(heading(1.0))));
            fake.start = Err(f);
            let mut s = wanted();
            let mut ds = DriverState::default();
            let t0 = Instant::now();
            drive(&s, &tx, &fake, t0, 0, &mut ds);
            // 反映前にもう一度フレームが来ても再 start しない。
            drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
            apply_all(&rx, &mut s);
            assert!(!s.wanted);
            assert!(matches!(s.view, CompassView::Failed(_)));
            assert_eq!(fake.counts(), (1, 0, 1));
        }
    }

    #[test]
    fn read_error_stops_then_fails() {
        let (tx, rx) = channel();
        let fake = Fake::new(Err(CompassFailure::PermissionDenied));
        let mut s = wanted();
        drive(
            &s,
            &tx,
            &fake,
            Instant::now(),
            0,
            &mut DriverState::default(),
        );
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts(), (1, 1, 1));
        assert_eq!(
            s.view,
            CompassView::Failed(CompassFailureView::PermissionDenied)
        );
    }

    #[test]
    fn no_value_for_timeout_stops_then_no_reading() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, CompassView::Waiting);
        let at = |ms: f64| t0 + Duration::from_secs_f64(ms / 1000.0);
        // 期限ちょうどでは失敗にならない。
        let mut ms = 500.0;
        while ms <= NO_READING_TIMEOUT_MS {
            drive(&s, &tx, &fake, at(ms), 0, &mut ds);
            ms += 500.0;
        }
        apply_all(&rx, &mut s);
        assert_eq!(s.view, CompassView::Waiting);
        assert_eq!(fake.counts().2, 0);
        drive(
            &s,
            &tx,
            &fake,
            at(NO_READING_TIMEOUT_MS + 200.0),
            0,
            &mut ds,
        );
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 1);
        assert_eq!(s.view, CompassView::Failed(CompassFailureView::NoReading));
    }

    #[test]
    fn long_gap_without_resume_still_times_out() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, CompassView::Failed(CompassFailureView::NoReading));
    }

    #[test]
    fn resume_event_restarts_wait() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        note_resumed();
        let epoch = RESUME_EPOCH.load(Ordering::SeqCst);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), epoch, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 0);
        assert_eq!(s.view, CompassView::Waiting);
    }

    #[test]
    fn resume_during_interval_is_not_lost() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + ms(2900), 0, &mut ds);
        // 復帰直後は間隔待ちで読まないが、通知は次の読み取りまで残る。
        drive(&s, &tx, &fake, t0 + ms(2950), 1, &mut ds);
        drive(&s, &tx, &fake, t0 + ms(3100), 1, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 0);
        assert_eq!(s.view, CompassView::Waiting);
    }

    #[test]
    fn background_pauses_wait() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        note_backgrounded();
        ds.background = BACKGROUND.load(Ordering::SeqCst);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), 0, &mut ds);
        note_resumed();
        ds.background = BACKGROUND.load(Ordering::SeqCst);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), 1, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 0);
        assert_eq!(s.view, CompassView::Waiting);
    }

    #[test]
    fn skips_read_within_interval_and_same_value_is_not_sent() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(Some(heading(90.2))));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(50), 0, &mut ds);
        assert_eq!(fake.counts().1, 1);
        // 表示上同じ整数度なら送らない。
        *fake.read.lock().unwrap() = Ok(Some(heading(89.9)));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn stale_session_completion_is_not_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(Some(heading(10.0))));
        let mut s = wanted();
        let mut ds = DriverState::default();
        drive(&s, &tx, &fake, Instant::now(), 0, &mut ds);
        // 完了が届く前に停止して再開始（世代 2）。
        stop(&mut s);
        start(&mut s);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, CompassView::Waiting);
    }

    #[test]
    fn new_session_restarts_after_stop_in_between() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(None));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        stop(&mut s);
        start(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        // 古い購読を止めてから開始し直す。
        assert_eq!((fake.counts().0, fake.counts().2), (2, 1));
    }

    #[test]
    fn unsupported_impl_never_becomes_live() {
        let (tx, rx) = channel();
        let mut s = wanted();
        drive(
            &s,
            &tx,
            &Unsupported,
            Instant::now(),
            0,
            &mut DriverState::default(),
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.view, CompassView::Failed(CompassFailureView::Unsupported));
    }
}
