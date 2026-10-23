//! Phase 11 の拡充ループ（親 #124、Issue #613）「行動認識（歩行・乗車等）」の effect ドライバ。
//! `fandhe_core::screens::activity_recognition_show`（端末依存ゼロ）の `wanted` の立ち上がりで
//! `fandhe_device::activity_recognition::ActivityRecognizer::start`、立ち下がりで `stop` を呼び、
//! 購読中は [`READ_INTERVAL`] ごとに `read` して結果を注入する（`registry.rs` の `effect` 区画から
//! 毎フレーム同期に呼ばれる。`compass.rs` と同型）。
//!
//! **間隔つき pull**: 最終読取から `READ_INTERVAL` 未満なら OS を読まず残り時間で再描画を予約する。
//! 読んだら送信を省く分岐でも次回を必ず予約する（入力が無いと egui が描画せずポーリングが止まる、
//! 契約表 4）。待機（購読なし）では予約しない。
//!
//! 失敗（開始失敗・読み取り失敗・期限切れ）を確定する前に必ず `stop()` する（契約表 3・9）。
//! 期限は単調時計（`Instant`）由来のミリ秒だけで測り、期限の起点はこのドライバの
//! [`FirstReadingDeadline`] だけが持つ（OS 側に別の期限を持たせない）。許可待ちは数えない。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。Windows は `windows_activity_recognition.rs`、
//! iOS は `ios_activity_recognition.rs`、Android（GMS 非依存の方針で保留）・
//! macOS・Linux は `UnsupportedActivityRecognizer`。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fandhe_core::screens::activity_recognition_show::{
    apply_failure, apply_poll, ActivityFailureView, ActivityKindView, ActivityPollView,
    ActivityReadingView, ActivityRecognitionShowState, ConfidenceView,
};
#[cfg(not(any(target_os = "windows", target_os = "ios")))]
use fandhe_device::activity_recognition::UnsupportedActivityRecognizer;
use fandhe_device::activity_recognition::{
    ActivityFailure, ActivityKind, ActivityPoll, ActivityRecognizer, Confidence,
    FirstReadingDeadline, PollKind, READ_INTERVAL,
};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す（プロセスで 1 個。購読を二重に作らないため
/// `OnceLock` で保持する）。
fn implementation() -> Arc<dyn ActivityRecognizer> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn ActivityRecognizer>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                super::windows_activity_recognition::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_activity_recognition::implementation()
            }
            #[cfg(not(any(target_os = "windows", target_os = "ios")))]
            {
                Arc::new(UnsupportedActivityRecognizer)
            }
        })
        .clone()
}

fn to_poll_view(p: ActivityPoll) -> ActivityPollView {
    match p {
        ActivityPoll::Waiting => ActivityPollView::Waiting,
        ActivityPoll::AwaitingPermission => ActivityPollView::AwaitingPermission,
        ActivityPoll::Reading(r) => ActivityPollView::Reading(ActivityReadingView {
            kind: match r.kind {
                ActivityKind::Unknown => ActivityKindView::Unknown,
                ActivityKind::Stationary => ActivityKindView::Stationary,
                ActivityKind::Walking => ActivityKindView::Walking,
                ActivityKind::Running => ActivityKindView::Running,
                ActivityKind::Cycling => ActivityKindView::Cycling,
                ActivityKind::Automotive => ActivityKindView::Automotive,
            },
            confidence: match r.confidence {
                Confidence::Low => ConfidenceView::Low,
                Confidence::Medium => ConfidenceView::Medium,
                Confidence::High => ConfidenceView::High,
            },
            seq: r.seq,
        }),
    }
}

/// 契約の失敗を画面の入力へ写す。OS 固有値は画面へ漏らさない（security.md A09）。
fn to_failure(f: ActivityFailure) -> ActivityFailureView {
    match f {
        ActivityFailure::Unsupported => ActivityFailureView::Unsupported,
        ActivityFailure::PermissionDenied => ActivityFailureView::PermissionDenied,
        ActivityFailure::Failed => ActivityFailureView::Failed,
    }
}

/// 送信済みの内容の記憶（同じ内容を二重に送らないため）。
type Sent = (u64, PollKind, Option<u64>);

/// driver の記憶（プロセス全体で 1 つ）。
#[derive(Default)]
struct DriverState {
    /// OS の購読を開始済みの世代。
    started_session: Option<u64>,
    /// 失敗を送信済みの世代（画面側が反映するまで再 `start` しない）。
    failed_session: Option<u64>,
    /// 購読を始めた時刻（経過時間の基準。単調時計だけを使う）。
    epoch: Option<Instant>,
    last_read: Option<Instant>,
    /// 直近に送った（世代, 結果の種別, 受信番号）。
    last_sent: Option<Sent>,
    /// 反映済みの復帰回数。
    resume_seen: u64,
    /// 背景中は読み取りも期限の計測も止める（復帰で起点を引き直す）。
    background: bool,
    deadline: FirstReadingDeadline,
}

static DRIVER: Mutex<Option<DriverState>> = Mutex::new(None);

fn send_failure(
    sender: &CompletionSender<ActivityRecognitionShowState>,
    session: u64,
    f: ActivityFailureView,
) {
    let _ = sender.send(Box::new(move |s: &mut ActivityRecognitionShowState| {
        apply_failure(s, session, f);
    }));
}

/// 背景からの復帰回数。プラットフォームが復帰イベントで増やし、ドライバは増えたときだけ
/// 期限の起点を引き直す。
static RESUME_EPOCH: AtomicU64 = AtomicU64::new(0);

/// 背景中か。背景中は期限を進めない。
static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// 背景遷移を記録する。iOS の実装（#614）が `ios_activity_recognition.rs` の C ABI 受け口から呼ぶ。
#[cfg(any(target_os = "ios", test))]
pub(crate) fn note_backgrounded() {
    BACKGROUND.store(true, Ordering::SeqCst);
}

/// 背景からの復帰を記録する（呼び元は [`note_backgrounded`] と同じ）。
/// 復帰回数を先に増やしてから背景フラグを下ろす（フラグが下りた時点で起点の引き直しが見える）。
#[cfg(any(target_os = "ios", test))]
pub(crate) fn note_resumed() {
    RESUME_EPOCH.fetch_add(1, Ordering::SeqCst);
    BACKGROUND.store(false, Ordering::SeqCst);
}

/// 開始・停止・間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &ActivityRecognitionShowState,
    sender: &CompletionSender<ActivityRecognitionShowState>,
    recognizer: &dyn ActivityRecognizer,
    now: Instant,
    resume_epoch: u64,
    ds: &mut DriverState,
) {
    if !state.wanted {
        // 立ち下がり: 購読を止める。待機では再描画を予約しない。
        if ds.started_session.take().is_some() {
            recognizer.stop();
        }
        ds.failed_session = None;
        ds.epoch = None;
        ds.last_read = None;
        ds.last_sent = None;
        ds.deadline.reset();
        return;
    }
    let session = state.session;
    if ds.failed_session == Some(session) {
        // 失敗を送信済み。画面側が反映するまで何もしない。
        return;
    }
    if ds.background {
        // 背景中は読まず期限も数えない。復帰後の最初の読み取りで起点を引き直す。
        return;
    }
    if ds.started_session != Some(session) {
        // 立ち上がり（または世代交代）: 古い購読を先に止めてから開始する。
        if ds.started_session.take().is_some() {
            recognizer.stop();
        }
        if let Err(e) = recognizer.start() {
            recognizer.stop();
            ds.failed_session = Some(session);
            send_failure(sender, session, to_failure(e));
            return;
        }
        ds.started_session = Some(session);
        ds.epoch = Some(now);
        ds.last_read = None;
        ds.last_sent = None;
        ds.deadline.begin(0.0);
        ds.resume_seen = resume_epoch;
    }

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

    let elapsed = ds.epoch.map_or(std::time::Duration::ZERO, |e| {
        now.saturating_duration_since(e)
    });
    let now_ms = elapsed.as_secs_f64() * 1000.0;
    if resumed {
        ds.deadline.resume(now_ms);
    }
    let poll = match recognizer.read() {
        Ok(p) => p,
        Err(e) => {
            recognizer.stop();
            ds.started_session = None;
            ds.failed_session = Some(session);
            send_failure(sender, session, to_failure(e));
            return;
        }
    };
    if ds.deadline.observe(PollKind::from(&poll), now_ms) {
        recognizer.stop();
        ds.started_session = None;
        ds.failed_session = Some(session);
        send_failure(sender, session, ActivityFailureView::NoReading);
        return;
    }

    // 同じ（世代, 種別, 受信番号）なら送らない。
    let seq = match &poll {
        ActivityPoll::Reading(r) => Some(r.seq),
        _ => None,
    };
    let key = (session, PollKind::from(&poll), seq);
    if ds.last_sent == Some(key) {
        return;
    }
    ds.last_sent = Some(key);
    let view = to_poll_view(poll);
    let secs = elapsed.as_secs();
    let _ = sender.send(Box::new(move |s: &mut ActivityRecognitionShowState| {
        apply_poll(s, session, view, secs);
    }));
}

/// `registry.rs` の `effect` 区画から `activity_recognition_show` 画面用に配線される effect ドライバ。
pub fn activity_recognition_show_effect(
    state: &ActivityRecognitionShowState,
    sender: &CompletionSender<ActivityRecognitionShowState>,
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
    use fandhe_core::screens::activity_recognition_show::{
        activity_text, start, stop, ActivityView,
    };
    use fandhe_device::activity_recognition::{
        ActivityReading, UnsupportedActivityRecognizer as Unsupported, NO_READING_TIMEOUT_MS,
    };
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<ActivityRecognitionShowState>>;

    fn channel() -> (CompletionSender<ActivityRecognitionShowState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply_all(rx: &Rx, s: &mut ActivityRecognitionShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    struct Fake {
        start: Result<(), ActivityFailure>,
        read: StdMutex<Result<ActivityPoll, ActivityFailure>>,
        starts: AtomicUsize,
        reads: AtomicUsize,
        stops: AtomicUsize,
    }

    impl Fake {
        fn new(read: Result<ActivityPoll, ActivityFailure>) -> Self {
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

    impl ActivityRecognizer for Fake {
        fn start(&self) -> Result<(), ActivityFailure> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.start
        }
        fn read(&self) -> Result<ActivityPoll, ActivityFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            *self.read.lock().unwrap()
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn reading(kind: ActivityKind, seq: u64) -> ActivityPoll {
        ActivityPoll::Reading(ActivityReading {
            kind,
            confidence: Confidence::High,
            seq,
        })
    }

    fn wanted() -> ActivityRecognitionShowState {
        let mut s = ActivityRecognitionShowState::default();
        start(&mut s);
        s
    }

    #[test]
    fn start_once_stop_once_and_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(ActivityKind::Walking, 1)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        assert_eq!(fake.counts(), (1, 2, 0));
        assert_eq!(activity_text(&s), "現在の行動: 歩行");
        stop(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 3, 0, &mut ds);
        assert_eq!(fake.counts(), (1, 2, 1));
    }

    #[test]
    fn idle_never_reads_or_starts() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ActivityPoll::Waiting));
        drive(
            &ActivityRecognitionShowState::default(),
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
            ActivityFailure::Unsupported,
            ActivityFailure::PermissionDenied,
            ActivityFailure::Failed,
        ] {
            let (tx, rx) = channel();
            let mut fake = Fake::new(Ok(ActivityPoll::Waiting));
            fake.start = Err(f);
            let mut s = wanted();
            let mut ds = DriverState::default();
            let t0 = Instant::now();
            drive(&s, &tx, &fake, t0, 0, &mut ds);
            drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
            apply_all(&rx, &mut s);
            assert!(!s.wanted);
            assert!(matches!(s.view, ActivityView::Failed(_)));
            assert_eq!(fake.counts(), (1, 0, 1));
        }
    }

    #[test]
    fn read_error_stops_then_fails() {
        let (tx, rx) = channel();
        let fake = Fake::new(Err(ActivityFailure::PermissionDenied));
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
            ActivityView::Failed(ActivityFailureView::PermissionDenied)
        );
    }

    #[test]
    fn no_value_for_timeout_stops_then_no_reading() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ActivityPoll::Waiting));
        let mut s = wanted();
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
        assert_eq!(s.view, ActivityView::Waiting);
        assert_eq!(fake.counts().2, 0);
        drive(
            &s,
            &tx,
            &fake,
            at(NO_READING_TIMEOUT_MS + 300.0),
            0,
            &mut ds,
        );
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 1);
        assert_eq!(s.view, ActivityView::Failed(ActivityFailureView::NoReading));
    }

    #[test]
    fn permission_wait_does_not_expire() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ActivityPoll::AwaitingPermission));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(60), 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, ActivityView::AwaitingPermission);
        assert_eq!(fake.counts().2, 0);
    }

    #[test]
    fn no_deadline_after_first_reading() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(ActivityKind::Stationary, 1)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        *fake.read.lock().unwrap() = Ok(ActivityPoll::Waiting);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(600), 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 0);
        // 値が消えた場合は値を出し続けず、判定中へ戻る。
        assert_eq!(s.view, ActivityView::Waiting);
    }

    #[test]
    fn background_pauses_and_resume_restarts_deadline() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ActivityPoll::Waiting));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        note_backgrounded();
        ds.background = BACKGROUND.load(Ordering::SeqCst);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), 0, &mut ds);
        assert_eq!(fake.counts().1, 1);
        note_resumed();
        ds.background = BACKGROUND.load(Ordering::SeqCst);
        let epoch = RESUME_EPOCH.load(Ordering::SeqCst);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), epoch, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 0);
        assert_eq!(s.view, ActivityView::Waiting);
    }

    #[test]
    fn skips_read_within_interval_and_same_seq_is_not_sent() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(ActivityKind::Walking, 7)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(50), 0, &mut ds);
        assert_eq!(fake.counts().1, 1);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        assert!(rx.try_recv().is_err());
        *fake.read.lock().unwrap() = Ok(reading(ActivityKind::Walking, 8));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, 0, &mut ds);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn stale_session_completion_is_dropped() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(ActivityKind::Running, 1)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        drive(&s, &tx, &fake, Instant::now(), 0, &mut ds);
        stop(&mut s);
        start(&mut s);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, ActivityView::Waiting);
    }

    #[test]
    fn new_session_stops_old_subscription_before_start() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(ActivityPoll::Waiting));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        stop(&mut s);
        start(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
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
        assert_eq!(
            s.view,
            ActivityView::Failed(ActivityFailureView::Unsupported)
        );
    }
}
