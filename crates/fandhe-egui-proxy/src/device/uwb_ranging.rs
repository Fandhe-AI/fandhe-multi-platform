//! Phase 11 の拡充ループ（親 #129、Issue #653）「UWB による距離・方向」の effect ドライバ。
//! `fandhe_core::screens::uwb_ranging_show`（端末依存ゼロ）の `wanted` の立ち上がりで
//! `fandhe_device::uwb_ranging::UwbRanging::start`、立ち下がりで `stop` を呼び、測距中は
//! [`READ_INTERVAL`] ごとに `read` して結果を注入する（`registry.rs` の `effect` 区画から
//! 毎フレーム同期に呼ばれる。`activity_recognition.rs` と同型）。端末の能力は画面が未取得の間だけ
//! 1 回読んで注入する。
//!
//! **間隔つき pull**: 最終読取から `READ_INTERVAL` 未満なら OS を読まず残り時間で再描画を予約する。
//! 読んだら送信を省く分岐でも次回を必ず予約する（契約表 4）。待機（測距なし）では予約しない。
//!
//! 失敗・終端（開始失敗・読み取り失敗・窓切れ・相手の終了・中断）を確定する前に必ず `stop()` する
//! （契約表 2・9）。窓は単調時計（`Instant`）由来のミリ秒だけで測り、起点を持つのはこのドライバの
//! [`ExchangeDeadline`] だけ（OS 側に別の期限を持たせない）。許可待ちは数えない。
//! 距離は [`validate_distance_m`] を通った値だけを、[`StaleTracker`] が新しいと判定した間だけ出す。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS は `ios_uwb_ranging.rs`、Android（GMS 依存で保留）・
//! macOS・Windows・Linux は `UnsupportedUwbRanging`。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::uwb_ranging_show::{
    apply_capability, apply_end, apply_failure, apply_poll, CapabilityView, EndReasonView,
    RangingPollView, SupportView, UwbFailureView, UwbRangingShowState,
};
#[cfg(not(target_os = "ios"))]
use fandhe_device::uwb_ranging::UnsupportedUwbRanging;
use fandhe_device::uwb_ranging::{
    to_centimeters, validate_distance_m, EndReason, ExchangeDeadline, Expiry, StaleTracker,
    Support, UwbCapabilities, UwbFailure, UwbPoll, UwbRanging, READ_INTERVAL,
};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す（プロセスで 1 個。MPC / `NISession` を二重に作らないため
/// `OnceLock` で保持する）。
fn implementation() -> Arc<dyn UwbRanging> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn UwbRanging>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "ios")]
            {
                super::ios_uwb_ranging::implementation()
            }
            #[cfg(not(target_os = "ios"))]
            {
                Arc::new(UnsupportedUwbRanging)
            }
        })
        .clone()
}

fn to_support(s: Support) -> SupportView {
    match s {
        Support::Yes => SupportView::Yes,
        Support::No => SupportView::No,
        Support::Unknown => SupportView::Unknown,
    }
}

/// 契約の失敗を画面の入力へ写す。OS 固有値は画面へ漏らさない（security.md A09）。
fn to_failure(f: UwbFailure) -> UwbFailureView {
    match f {
        UwbFailure::UnsupportedOnOs => UwbFailureView::UnsupportedOnOs,
        UwbFailure::NotCapable => UwbFailureView::NotCapable,
        UwbFailure::PermissionDenied => UwbFailureView::PermissionDenied,
        UwbFailure::Timeout => UwbFailureView::Timeout,
        UwbFailure::Failed => UwbFailureView::Failed,
    }
}

fn to_capability(r: Result<UwbCapabilities, UwbFailure>) -> CapabilityView {
    match r {
        Ok(c) => CapabilityView::Known {
            distance: to_support(c.distance),
            direction: to_support(c.direction),
        },
        Err(f) => CapabilityView::Unavailable(to_failure(f)),
    }
}

fn to_end(r: EndReason) -> EndReasonView {
    match r {
        EndReason::PeerEnded => EndReasonView::PeerEnded,
        EndReason::Interrupted => EndReasonView::Interrupted,
        EndReason::NoPeer => EndReasonView::NoPeer,
    }
}

/// 送信済みの内容の種別（同じ内容を二重に送らないため）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SentKind {
    Searching,
    Exchanging,
    AwaitingPermission,
    Measured,
    NoMeasurement,
}

/// 送信済みの内容の記憶（世代, 種別, 受信番号）。
type Sent = (u64, SentKind, Option<u64>);

/// driver の記憶（プロセス全体で 1 つ）。
#[derive(Default)]
struct DriverState {
    /// 読み出し済みの能力（OS を繰り返し読まないため）。
    capability: Option<CapabilityView>,
    /// OS の測距を開始済みの世代。
    started_session: Option<u64>,
    /// 終端・失敗を送信済みの世代（画面側が反映するまで再 `start` しない）。
    finished_session: Option<u64>,
    /// 開始した時刻（経過時間の基準。単調時計だけを使う）。
    epoch: Option<Instant>,
    last_read: Option<Instant>,
    last_sent: Option<Sent>,
    deadline: ExchangeDeadline,
    stale: StaleTracker,
}

static DRIVER: Mutex<Option<DriverState>> = Mutex::new(None);

impl DriverState {
    /// 待機へ戻るときに測距ごとの記憶を捨てる。
    fn clear_session(&mut self) {
        self.started_session = None;
        self.finished_session = None;
        self.epoch = None;
        self.last_read = None;
        self.last_sent = None;
        self.deadline.reset();
        self.stale.reset();
    }
}

type Sender = CompletionSender<UwbRangingShowState>;

/// 失敗を確定する。`stop()` と開始済みの記憶の解除を済ませてから送る（結果を届ける前に状態を確定する）。
fn finish_failure(
    ds: &mut DriverState,
    sender: &Sender,
    imp: &dyn UwbRanging,
    session: u64,
    f: UwbFailureView,
) {
    imp.stop();
    ds.started_session = None;
    ds.finished_session = Some(session);
    let _ = sender.send(Box::new(move |s: &mut UwbRangingShowState| {
        apply_failure(s, session, f);
    }));
}

/// 正常終了を確定する（順序は [`finish_failure`] と同じ）。
fn finish_end(
    ds: &mut DriverState,
    sender: &Sender,
    imp: &dyn UwbRanging,
    session: u64,
    r: EndReasonView,
) {
    imp.stop();
    ds.started_session = None;
    ds.finished_session = Some(session);
    let _ = sender.send(Box::new(move |s: &mut UwbRangingShowState| {
        apply_end(s, session, r);
    }));
}

/// 能力の注入・開始・停止・間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &UwbRangingShowState,
    sender: &Sender,
    imp: &dyn UwbRanging,
    now: Instant,
    ds: &mut DriverState,
) {
    // 画面が能力を未取得の間だけ注入する。OS は 1 回しか読まない。
    if state.capability == CapabilityView::NotRead {
        let cap = *ds
            .capability
            .get_or_insert_with(|| to_capability(imp.capabilities()));
        let _ = sender.send(Box::new(move |s: &mut UwbRangingShowState| {
            apply_capability(s, cap);
        }));
    }

    if !state.wanted {
        // 立ち下がり: 止める。待機では再描画を予約しない。
        if ds.started_session.is_some() {
            imp.stop();
        }
        ds.clear_session();
        return;
    }
    let session = state.session;
    if ds.finished_session == Some(session) {
        // 終端・失敗を送信済み。画面側が反映するまで何もしない。
        return;
    }
    if ds.started_session != Some(session) {
        // 立ち上がり（または世代交代）: 古い測距を先に止めてから開始する。
        if ds.started_session.take().is_some() {
            imp.stop();
        }
        if let Err(e) = imp.start() {
            finish_failure(ds, sender, imp, session, to_failure(e));
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

    let now_ms = ds
        .epoch
        .map_or(Duration::ZERO, |e| now.saturating_duration_since(e))
        .as_secs_f64()
        * 1000.0;
    let poll = match imp.read() {
        Ok(p) => p,
        Err(e) => {
            finish_failure(ds, sender, imp, session, to_failure(e));
            return;
        }
    };
    match poll {
        UwbPoll::PeerEnded => {
            return finish_end(ds, sender, imp, session, to_end(EndReason::PeerEnded))
        }
        UwbPoll::Interrupted => {
            return finish_end(ds, sender, imp, session, to_end(EndReason::Interrupted))
        }
        _ => {}
    }
    if let Some(phase) = poll.exchange_phase() {
        match ds.deadline.observe(phase, now_ms) {
            Some(Expiry::NoPeer) => {
                return finish_end(ds, sender, imp, session, to_end(EndReason::NoPeer))
            }
            Some(Expiry::Timeout) => {
                return finish_failure(ds, sender, imp, session, UwbFailureView::Timeout)
            }
            None => {}
        }
    }

    let (kind, seq, view) = match poll {
        UwbPoll::Searching => (SentKind::Searching, None, RangingPollView::Searching),
        UwbPoll::Exchanging => (SentKind::Exchanging, None, RangingPollView::Exchanging),
        UwbPoll::AwaitingPermission => (
            SentKind::AwaitingPermission,
            None,
            RangingPollView::AwaitingPermission,
        ),
        UwbPoll::NoMeasurement => (
            SentKind::NoMeasurement,
            None,
            RangingPollView::NoMeasurement,
        ),
        UwbPoll::Measured(sample) => {
            // 範囲外・NaN と古い値は距離なしに落とす（0 m や前回値で代用しない）。
            match validate_distance_m(sample.distance_m) {
                Some(m) if ds.stale.observe(sample.seq, now_ms) => (
                    SentKind::Measured,
                    Some(sample.seq),
                    RangingPollView::Measured {
                        cm: to_centimeters(m),
                        seq: sample.seq,
                    },
                ),
                _ => (
                    SentKind::NoMeasurement,
                    None,
                    RangingPollView::NoMeasurement,
                ),
            }
        }
        // 終端は上で処理済み。
        UwbPoll::PeerEnded | UwbPoll::Interrupted => return,
    };

    // 同じ（世代, 種別, 受信番号）なら送らない。
    let key = (session, kind, seq);
    if ds.last_sent == Some(key) {
        return;
    }
    ds.last_sent = Some(key);
    let _ = sender.send(Box::new(move |s: &mut UwbRangingShowState| {
        apply_poll(s, session, view);
    }));
}

/// `registry.rs` の `effect` 区画から `uwb_ranging_show` 画面用に配線される effect ドライバ。
pub fn uwb_ranging_show_effect(state: &UwbRangingShowState, sender: &Sender) {
    let mut guard = DRIVER.lock().unwrap_or_else(|p| p.into_inner());
    let ds = guard.get_or_insert_with(DriverState::default);
    drive(state, sender, implementation().as_ref(), Instant::now(), ds);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::uwb_ranging_show::{start, stop, RangingView};
    use fandhe_device::uwb_ranging::{
        RangeSample, UnsupportedUwbRanging as Unsupported, EXCHANGE_WINDOW_MS, MAX_DISTANCE_M,
        STALE_AFTER_MS,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<UwbRangingShowState>>;

    fn channel() -> (Sender, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply_all(rx: &Rx, s: &mut UwbRangingShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    struct Fake {
        caps: Result<UwbCapabilities, UwbFailure>,
        start: Result<(), UwbFailure>,
        read: StdMutex<Result<UwbPoll, UwbFailure>>,
        cap_calls: AtomicUsize,
        starts: AtomicUsize,
        reads: AtomicUsize,
        stops: AtomicUsize,
    }

    impl Fake {
        fn new(read: Result<UwbPoll, UwbFailure>) -> Self {
            Self {
                caps: Ok(UwbCapabilities {
                    distance: Support::Yes,
                    direction: Support::No,
                }),
                start: Ok(()),
                read: StdMutex::new(read),
                cap_calls: AtomicUsize::new(0),
                starts: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
            }
        }
        /// （start, read, stop）の回数。
        fn counts(&self) -> (usize, usize, usize) {
            (
                self.starts.load(Ordering::SeqCst),
                self.reads.load(Ordering::SeqCst),
                self.stops.load(Ordering::SeqCst),
            )
        }
        fn set(&self, r: Result<UwbPoll, UwbFailure>) {
            *self.read.lock().unwrap() = r;
        }
    }

    impl UwbRanging for Fake {
        fn capabilities(&self) -> Result<UwbCapabilities, UwbFailure> {
            self.cap_calls.fetch_add(1, Ordering::SeqCst);
            self.caps
        }
        fn start(&self) -> Result<(), UwbFailure> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.start
        }
        fn read(&self) -> Result<UwbPoll, UwbFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            *self.read.lock().unwrap()
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn measured(m: f32, seq: u64) -> UwbPoll {
        UwbPoll::Measured(RangeSample { distance_m: m, seq })
    }

    fn wanted() -> UwbRangingShowState {
        let mut s = UwbRangingShowState::default();
        start(&mut s);
        s
    }

    fn at(t0: Instant, ms: f64) -> Instant {
        t0 + Duration::from_secs_f64(ms / 1000.0)
    }

    #[test]
    fn capability_is_read_once_and_reaches_screen() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(UwbPoll::Searching));
        let mut s = UwbRangingShowState::default();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
        assert_eq!(fake.cap_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            s.capability,
            CapabilityView::Known {
                distance: SupportView::Yes,
                direction: SupportView::No
            }
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn capability_failure_reaches_screen() {
        let (tx, rx) = channel();
        let mut fake = Fake::new(Ok(UwbPoll::Searching));
        fake.caps = Err(UwbFailure::NotCapable);
        let mut s = UwbRangingShowState::default();
        drive(&s, &tx, &fake, Instant::now(), &mut DriverState::default());
        apply_all(&rx, &mut s);
        assert_eq!(
            s.capability,
            CapabilityView::Unavailable(UwbFailureView::NotCapable)
        );
    }

    #[test]
    fn start_once_stop_once_and_measured() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(measured(1.234, 1)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
        assert_eq!(fake.counts(), (1, 2, 0));
        assert_eq!(s.view, RangingView::Measured { cm: 123, seq: 1 });
        stop(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, &mut ds);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 3, &mut ds);
        assert_eq!(fake.counts(), (1, 2, 1));
    }

    #[test]
    fn idle_never_starts_or_reads() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(UwbPoll::Searching));
        drive(
            &UwbRangingShowState::default(),
            &tx,
            &fake,
            Instant::now(),
            &mut DriverState::default(),
        );
        assert_eq!(fake.counts(), (0, 0, 0));
    }

    #[test]
    fn start_failures_stop_then_fail_and_do_not_retry() {
        for f in [
            UwbFailure::NotCapable,
            UwbFailure::PermissionDenied,
            UwbFailure::UnsupportedOnOs,
        ] {
            let (tx, rx) = channel();
            let mut fake = Fake::new(Ok(UwbPoll::Searching));
            fake.start = Err(f);
            let mut s = wanted();
            let mut ds = DriverState::default();
            let t0 = Instant::now();
            drive(&s, &tx, &fake, t0, &mut ds);
            drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
            apply_all(&rx, &mut s);
            assert!(!s.wanted);
            assert_eq!(s.view, RangingView::Failed(to_failure(f)));
            assert_eq!(fake.counts(), (1, 0, 1));
        }
    }

    #[test]
    fn read_error_stops_then_fails() {
        let (tx, rx) = channel();
        let fake = Fake::new(Err(UwbFailure::Failed));
        let mut s = wanted();
        drive(&s, &tx, &fake, Instant::now(), &mut DriverState::default());
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts(), (1, 1, 1));
        assert_eq!(s.view, RangingView::Failed(UwbFailureView::Failed));
    }

    #[test]
    fn searching_past_window_is_no_peer_not_failure() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(UwbPoll::Searching));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        drive(&s, &tx, &fake, at(t0, EXCHANGE_WINDOW_MS), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, RangingView::Searching);
        assert_eq!(fake.counts().2, 0);
        drive(&s, &tx, &fake, at(t0, EXCHANGE_WINDOW_MS + 200.0), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 1);
        assert_eq!(s.view, RangingView::Ended(EndReasonView::NoPeer));
        assert!(!s.wanted);
    }

    #[test]
    fn exchanging_past_window_is_timeout_failure() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(UwbPoll::Exchanging));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        drive(&s, &tx, &fake, at(t0, EXCHANGE_WINDOW_MS + 200.0), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 1);
        assert_eq!(s.view, RangingView::Failed(UwbFailureView::Timeout));
    }

    #[test]
    fn permission_wait_does_not_expire() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(UwbPoll::AwaitingPermission));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(120), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, RangingView::AwaitingPermission);
        assert_eq!(fake.counts().2, 0);
    }

    #[test]
    fn measured_after_window_is_not_accepted() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(UwbPoll::Exchanging));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        fake.set(Ok(measured(1.0, 1)));
        drive(&s, &tx, &fake, at(t0, EXCHANGE_WINDOW_MS + 200.0), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, RangingView::Failed(UwbFailureView::Timeout));
    }

    #[test]
    fn no_deadline_once_ranging() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(measured(1.0, 1)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        fake.set(Ok(UwbPoll::NoMeasurement));
        drive(&s, &tx, &fake, t0 + Duration::from_secs(600), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 0);
        assert_eq!(s.view, RangingView::NoMeasurement);
    }

    #[test]
    fn peer_ended_and_interrupted_stop_then_end() {
        for (poll, want) in [
            (UwbPoll::PeerEnded, EndReasonView::PeerEnded),
            (UwbPoll::Interrupted, EndReasonView::Interrupted),
        ] {
            let (tx, rx) = channel();
            let fake = Fake::new(Ok(poll));
            let mut s = wanted();
            let mut ds = DriverState::default();
            let t0 = Instant::now();
            drive(&s, &tx, &fake, t0, &mut ds);
            apply_all(&rx, &mut s);
            assert_eq!(fake.counts(), (1, 1, 1));
            assert_eq!(s.view, RangingView::Ended(want));
            // 終端の後は再 start / 再 read しない。
            drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
            assert_eq!(fake.counts(), (1, 1, 1));
        }
    }

    #[test]
    fn invalid_distance_becomes_no_measurement() {
        for bad in [f32::NAN, f32::INFINITY, -1.0, MAX_DISTANCE_M + 1.0] {
            let (tx, rx) = channel();
            let fake = Fake::new(Ok(measured(bad, 1)));
            let mut s = wanted();
            drive(&s, &tx, &fake, Instant::now(), &mut DriverState::default());
            apply_all(&rx, &mut s);
            assert_eq!(s.view, RangingView::NoMeasurement);
        }
    }

    #[test]
    fn stale_value_becomes_no_measurement() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(measured(2.0, 7)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, RangingView::Measured { cm: 200, seq: 7 });
        drive(&s, &tx, &fake, at(t0, STALE_AFTER_MS + 200.0), &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, RangingView::NoMeasurement);
    }

    #[test]
    fn skips_read_within_interval_and_same_content_is_not_sent() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(measured(1.0, 7)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(50), &mut ds);
        assert_eq!(fake.counts().1, 1);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut ds);
        assert!(rx.try_recv().is_err());
        fake.set(Ok(measured(1.1, 8)));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, &mut ds);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn stale_session_completion_is_dropped() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(measured(1.0, 1)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        drive(&s, &tx, &fake, Instant::now(), &mut ds);
        stop(&mut s);
        start(&mut s);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, RangingView::Searching);
    }

    #[test]
    fn new_session_stops_old_before_start() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(UwbPoll::Searching));
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
    fn unsupported_impl_never_succeeds() {
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
        assert_eq!(
            s.capability,
            CapabilityView::Unavailable(UwbFailureView::UnsupportedOnOs)
        );
        assert_eq!(s.view, RangingView::Failed(UwbFailureView::UnsupportedOnOs));
    }
}
