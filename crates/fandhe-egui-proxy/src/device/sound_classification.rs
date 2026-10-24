//! Phase 11 の拡充ループ（親 #142、Issue #725）「環境音の分類」の effect ドライバ。
//! `fandhe_core::screens::sound_classification_show`（端末依存ゼロ）の `wanted` の立ち上がりで
//! `fandhe_device::sound_classification::SoundClassifier::start`、立ち下がりで `stop` を呼び、
//! 聞き取り中は [`READ_INTERVAL`] ごとに `read` して結果を注入する（`registry.rs` の `effect` 区画から
//! 毎フレーム同期に呼ばれる。`activity_recognition.rs` と同型）。
//!
//! **間隔つき pull**: 最終読取から `READ_INTERVAL` 未満なら OS を読まず残り時間で再描画を予約する。
//! 読んだら送信を省く分岐でも次回を必ず予約する（入力が無いと egui が描画せずポーリングが止まる、
//! 契約表 4）。待機（聞き取りなし）では予約しない。
//!
//! 失敗（開始失敗・読み取り失敗・期限切れ）と中断を確定する前に必ず `stop()` する（契約表 3・9）。
//! 期限と古さの判定は単調時計（`Instant`）由来のミリ秒だけで測り、起点はこのドライバの
//! [`FirstResultDeadline`] / [`StaleTracker`] だけが持つ（OS 側に別の期限を持たせない）。
//! 許可待ちは期限に数えない。結果の番号が [`STALE_AFTER_MS`] を超えて進まなければ、古い結果を
//! 消して「聞き取り中」を送る。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。macOS は `apple_sound_classification.rs`、
//! iOS は `ios_sound_classification.rs`（b-ios まではスタブ）、それ以外は `UnsupportedSoundClassifier`。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use fandhe_core::screens::sound_classification_show::{
    apply_failure, apply_interrupted, apply_poll, ClassificationView, InterruptReasonView,
    ResultView, SoundClassificationShowState, SoundFailureView, SoundPollView,
};
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
use fandhe_device::sound_classification::UnsupportedSoundClassifier;
use fandhe_device::sound_classification::{
    to_percent, FirstResultDeadline, InterruptReason, PollKind, SoundClassifier, SoundFailure,
    SoundPoll, StaleTracker, READ_INTERVAL,
};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す（プロセスで 1 個。聞き取りを二重に作らないため
/// `OnceLock` で保持する）。
fn implementation() -> Arc<dyn SoundClassifier> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn SoundClassifier>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                super::apple_sound_classification::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_sound_classification::implementation()
            }
            #[cfg(not(any(target_os = "macos", target_os = "ios")))]
            {
                Arc::new(UnsupportedSoundClassifier)
            }
        })
        .clone()
}

fn to_interrupt_view(r: InterruptReason) -> InterruptReasonView {
    match r {
        InterruptReason::Background => InterruptReasonView::Background,
        InterruptReason::AudioInterruption => InterruptReasonView::AudioInterruption,
        InterruptReason::InputChanged => InterruptReasonView::InputChanged,
    }
}

/// 契約の失敗を画面の入力へ写す。OS 固有値は画面へ漏らさない（security.md A09）。
fn to_failure(f: SoundFailure) -> SoundFailureView {
    match f {
        SoundFailure::Unsupported => SoundFailureView::Unsupported,
        SoundFailure::PermissionDenied => SoundFailureView::PermissionDenied,
        SoundFailure::NoMicrophone => SoundFailureView::NoMicrophone,
        SoundFailure::Failed => SoundFailureView::Failed,
    }
}

/// 結果の写し。候補が空の結果は作らない（`None`）。
fn to_result_view(
    r: &fandhe_device::sound_classification::ClassificationResult,
) -> Option<ResultView> {
    if r.top.is_empty() {
        return None;
    }
    Some(ResultView {
        top: r
            .top
            .iter()
            .map(|c| ClassificationView {
                label: c.label.clone(),
                percent: to_percent(c.confidence),
            })
            .collect(),
        seq: r.seq,
    })
}

/// 送信済みの内容の記憶（同じ内容を二重に送らないため）。
type Sent = (u64, PollKind, Option<u64>);

/// driver の記憶（プロセス全体で 1 つ）。
#[derive(Default)]
struct DriverState {
    /// OS の聞き取りを開始済みの世代。
    started_session: Option<u64>,
    /// 失敗・中断を送信済みの世代（画面側が反映するまで再 `start` しない）。
    ended_session: Option<u64>,
    /// 聞き取りを始めた時刻（経過時間の基準。単調時計だけを使う）。
    epoch: Option<Instant>,
    last_read: Option<Instant>,
    /// 直近に送った（世代, 結果の種別, 結果の番号）。
    last_sent: Option<Sent>,
    deadline: FirstResultDeadline,
    stale: StaleTracker,
}

static DRIVER: Mutex<Option<DriverState>> = Mutex::new(None);

fn send_failure(
    sender: &CompletionSender<SoundClassificationShowState>,
    session: u64,
    f: SoundFailureView,
) {
    let _ = sender.send(Box::new(move |s: &mut SoundClassificationShowState| {
        apply_failure(s, session, f);
    }));
}

/// 開始・停止・間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &SoundClassificationShowState,
    sender: &CompletionSender<SoundClassificationShowState>,
    classifier: &dyn SoundClassifier,
    now: Instant,
    ds: &mut DriverState,
) {
    if !state.wanted {
        // 立ち下がり: 聞き取りを止める。待機では再描画を予約しない。
        if ds.started_session.take().is_some() {
            classifier.stop();
        }
        ds.ended_session = None;
        ds.epoch = None;
        ds.last_read = None;
        ds.last_sent = None;
        ds.deadline.reset();
        ds.stale.reset();
        return;
    }
    let session = state.session;
    if ds.ended_session == Some(session) {
        // 失敗・中断を送信済み。画面側が反映するまで何もしない。
        return;
    }
    if ds.started_session != Some(session) {
        // 立ち上がり（または世代交代）: 古い聞き取りを先に止めてから開始する。
        if ds.started_session.take().is_some() {
            classifier.stop();
        }
        if let Err(e) = classifier.start() {
            classifier.stop();
            ds.ended_session = Some(session);
            send_failure(sender, session, to_failure(e));
            return;
        }
        ds.started_session = Some(session);
        ds.epoch = Some(now);
        ds.last_read = None;
        ds.last_sent = None;
        ds.deadline.begin(0.0);
        ds.stale.reset();
    }

    if let Some(prev) = ds.last_read {
        let elapsed = now.saturating_duration_since(prev);
        if elapsed < READ_INTERVAL {
            sender.request_repaint_after(READ_INTERVAL - elapsed);
            return;
        }
    }
    ds.last_read = Some(now);
    // 送信を省略する分岐でも次回の読み取りを予約する。
    sender.request_repaint_after(READ_INTERVAL);

    let now_ms = ds.epoch.map_or(0.0, |e| {
        now.saturating_duration_since(e).as_secs_f64() * 1000.0
    });
    let poll = match classifier.read() {
        Ok(p) => p,
        Err(e) => {
            classifier.stop();
            ds.started_session = None;
            ds.ended_session = Some(session);
            send_failure(sender, session, to_failure(e));
            return;
        }
    };

    // 中断は終端: 資源を解放してから確定する。自動では再開しない。
    if let SoundPoll::Interrupted(reason) = &poll {
        let view = to_interrupt_view(*reason);
        classifier.stop();
        ds.started_session = None;
        ds.ended_session = Some(session);
        let _ = sender.send(Box::new(move |s: &mut SoundClassificationShowState| {
            apply_interrupted(s, session, view);
        }));
        return;
    }

    // 候補が空の結果は結果なし扱い（嘘の結果を作らない）。
    let poll = match poll {
        SoundPoll::Result(r) if to_result_view(&r).is_none() => SoundPoll::Listening,
        p => p,
    };
    if ds.deadline.observe(PollKind::from(&poll), now_ms) {
        classifier.stop();
        ds.started_session = None;
        ds.ended_session = Some(session);
        send_failure(sender, session, SoundFailureView::NoResult);
        return;
    }

    // 結果の番号が進まないまま古くなったら、結果を消して聞き取り中へ戻す。
    let poll = match poll {
        SoundPoll::Result(r) => {
            if ds.stale.observe(r.seq, now_ms) {
                SoundPoll::Result(r)
            } else {
                SoundPoll::Listening
            }
        }
        p => p,
    };

    let seq = match &poll {
        SoundPoll::Result(r) => Some(r.seq),
        _ => None,
    };
    let key = (session, PollKind::from(&poll), seq);
    if ds.last_sent == Some(key) {
        return;
    }
    ds.last_sent = Some(key);
    let view = match poll {
        SoundPoll::Idle => SoundPollView::Idle,
        SoundPoll::AwaitingPermission => SoundPollView::AwaitingPermission,
        SoundPoll::Listening => SoundPollView::Listening,
        SoundPoll::Result(r) => match to_result_view(&r) {
            Some(v) => SoundPollView::Result(v),
            None => SoundPollView::Listening,
        },
        // 上で処理済み。
        SoundPoll::Interrupted(_) => return,
    };
    let _ = sender.send(Box::new(move |s: &mut SoundClassificationShowState| {
        apply_poll(s, session, view);
    }));
}

/// `registry.rs` の `effect` 区画から `sound_classification_show` 画面用に配線される effect ドライバ。
pub fn sound_classification_show_effect(
    state: &SoundClassificationShowState,
    sender: &CompletionSender<SoundClassificationShowState>,
) {
    let mut guard = DRIVER.lock().unwrap_or_else(|p| p.into_inner());
    let ds = guard.get_or_insert_with(DriverState::default);
    drive(state, sender, implementation().as_ref(), Instant::now(), ds);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::sound_classification_show::{
        result_text, start, status_text, stop, SoundView, FAILED_TEXT, LISTENING_TEXT,
    };
    use fandhe_device::sound_classification::{
        Classification, ClassificationResult, UnsupportedSoundClassifier as Unsupported,
        FIRST_RESULT_TIMEOUT_MS, STALE_AFTER_MS,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<SoundClassificationShowState>>;

    fn channel() -> (CompletionSender<SoundClassificationShowState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply_all(rx: &Rx, s: &mut SoundClassificationShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    struct Fake {
        start: Result<(), SoundFailure>,
        read: StdMutex<Result<SoundPoll, SoundFailure>>,
        starts: AtomicUsize,
        reads: AtomicUsize,
        stops: AtomicUsize,
    }

    impl Fake {
        fn new(read: Result<SoundPoll, SoundFailure>) -> Self {
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

    impl SoundClassifier for Fake {
        fn start(&self) -> Result<(), SoundFailure> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.start
        }
        fn read(&self) -> Result<SoundPoll, SoundFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.read.lock().unwrap().clone()
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn result(seq: u64, label: &str, conf: f64) -> SoundPoll {
        SoundPoll::Result(ClassificationResult {
            top: vec![Classification {
                label: label.to_string(),
                confidence: conf,
            }],
            seq,
        })
    }

    fn wanted() -> SoundClassificationShowState {
        let mut s = SoundClassificationShowState::default();
        start(&mut s);
        s
    }

    #[test]
    fn start_once_stop_once_and_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(result(1, "dog_bark", 0.87)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
        assert_eq!(fake.counts(), (1, 2, 0));
        assert_eq!(result_text(&s), "dog_bark — 87%");
        stop(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, &mut ds);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 3, &mut ds);
        // キャンセル: stop は 1 回だけ呼ばれ、結果は残らない。
        assert_eq!(fake.counts(), (1, 2, 1));
        assert_eq!(s.view, SoundView::Idle);
        assert_eq!(result_text(&s), "");
    }

    #[test]
    fn idle_never_reads_or_starts() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(SoundPoll::Listening));
        drive(
            &SoundClassificationShowState::default(),
            &tx,
            &fake,
            Instant::now(),
            &mut DriverState::default(),
        );
        assert_eq!(fake.counts(), (0, 0, 0));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn start_failures_never_become_live_and_do_not_retry() {
        for f in [
            SoundFailure::Unsupported,
            SoundFailure::PermissionDenied,
            SoundFailure::NoMicrophone,
            SoundFailure::Failed,
        ] {
            let (tx, rx) = channel();
            let mut fake = Fake::new(Ok(SoundPoll::Listening));
            fake.start = Err(f);
            let mut s = wanted();
            let mut ds = DriverState::default();
            let t0 = Instant::now();
            drive(&s, &tx, &fake, t0, &mut ds);
            drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
            apply_all(&rx, &mut s);
            assert!(!s.wanted);
            assert!(matches!(s.view, SoundView::Failed(_)));
            assert_eq!(result_text(&s), "");
            assert_eq!(fake.counts(), (1, 0, 1));
        }
    }

    #[test]
    fn permission_denied_on_read_stops_then_fails() {
        let (tx, rx) = channel();
        let fake = Fake::new(Err(SoundFailure::PermissionDenied));
        let mut s = wanted();
        drive(&s, &tx, &fake, Instant::now(), &mut DriverState::default());
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts(), (1, 1, 1));
        assert_eq!(
            s.view,
            SoundView::Failed(SoundFailureView::PermissionDenied)
        );
    }

    #[test]
    fn no_result_for_timeout_stops_then_no_result() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(SoundPoll::Listening));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        let at = |ms: f64| t0 + Duration::from_secs_f64(ms / 1000.0);
        drive(&s, &tx, &fake, t0, &mut ds);
        let mut ms = 500.0;
        while ms <= FIRST_RESULT_TIMEOUT_MS {
            drive(&s, &tx, &fake, at(ms), &mut ds);
            ms += 500.0;
        }
        apply_all(&rx, &mut s);
        assert_eq!(s.view, SoundView::Listening);
        assert_eq!(fake.counts().2, 0);
        drive(&s, &tx, &fake, at(FIRST_RESULT_TIMEOUT_MS + 300.0), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 1);
        assert_eq!(s.view, SoundView::Failed(SoundFailureView::NoResult));
    }

    #[test]
    fn permission_wait_does_not_expire() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(SoundPoll::AwaitingPermission));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(60), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, SoundView::AwaitingPermission);
        assert_eq!(fake.counts().2, 0);
    }

    #[test]
    fn interruption_stops_and_does_not_restart() {
        for r in [
            InterruptReason::Background,
            InterruptReason::AudioInterruption,
            InterruptReason::InputChanged,
        ] {
            let (tx, rx) = channel();
            let fake = Fake::new(Ok(SoundPoll::Interrupted(r)));
            let mut s = wanted();
            let mut ds = DriverState::default();
            let t0 = Instant::now();
            drive(&s, &tx, &fake, t0, &mut ds);
            drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
            drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, &mut ds);
            apply_all(&rx, &mut s);
            assert!(!s.wanted);
            assert_eq!(s.view, SoundView::Interrupted(to_interrupt_view(r)));
            // 開始 1 回・停止 1 回のまま。自動では再開しない。
            assert_eq!(fake.counts(), (1, 1, 1));
            drive(&s, &tx, &fake, t0 + READ_INTERVAL * 3, &mut ds);
            assert_eq!(fake.counts().0, 1);
        }
    }

    #[test]
    fn empty_result_is_listening_not_a_result() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(SoundPoll::Result(ClassificationResult {
            top: vec![],
            seq: 1,
        })));
        let mut s = wanted();
        drive(&s, &tx, &fake, Instant::now(), &mut DriverState::default());
        apply_all(&rx, &mut s);
        assert_eq!(s.view, SoundView::Listening);
        assert_eq!(result_text(&s), "");
    }

    #[test]
    fn stale_result_is_dropped_back_to_listening() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(result(1, "speech", 0.9)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        let at = |ms: f64| t0 + Duration::from_secs_f64(ms / 1000.0);
        drive(&s, &tx, &fake, t0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(result_text(&s), "speech — 90%");
        drive(&s, &tx, &fake, at(STALE_AFTER_MS + 300.0), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, SoundView::Listening);
        assert_eq!(status_text(&s), LISTENING_TEXT);
        // 番号が進めば再び結果を出す。
        *fake.read.lock().unwrap() = Ok(result(2, "music", 0.6));
        drive(&s, &tx, &fake, at(STALE_AFTER_MS + 600.0), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(result_text(&s), "music — 60%");
    }

    #[test]
    fn skips_read_within_interval_and_same_seq_is_not_sent() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(result(7, "a", 0.5)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(50), &mut ds);
        assert_eq!(fake.counts().1, 1);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
        assert!(rx.try_recv().is_err());
        *fake.read.lock().unwrap() = Ok(result(8, "a", 0.5));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, &mut ds);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn stale_session_completion_is_dropped() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(result(1, "a", 0.5)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        drive(&s, &tx, &fake, Instant::now(), &mut ds);
        stop(&mut s);
        start(&mut s);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, SoundView::Listening);
        assert_eq!(result_text(&s), "");
    }

    #[test]
    fn new_session_stops_old_listening_before_start() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(SoundPoll::Listening));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        stop(&mut s);
        start(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
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
            &mut DriverState::default(),
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.view, SoundView::Failed(SoundFailureView::Unsupported));
        assert_ne!(status_text(&s), FAILED_TEXT);
    }
}
