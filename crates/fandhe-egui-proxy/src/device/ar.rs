//! Phase 11 の拡充ループ（親 #90、Issue #307）「AR 表示」の effect ドライバ。
//! `fandhe_core::screens::ar_show`（端末依存ゼロ）の `wanted` の立ち上がりで
//! `fandhe_device::ar::ArTracking::start`、立ち下がりで `stop` を呼び、実行中は
//! [`READ_INTERVAL`] ごとに `read` して結果を注入する（`registry.rs` の `effect` 区画から
//! 毎フレーム同期に呼ばれる）。`compass.rs` と同型の間隔つき pull。
//!
//! 読んだら送信を省く分岐でも次回を必ず予約する（契約表 4）。失敗（開始失敗・読み取り失敗・期限切れ）
//! を確定する前に必ず `stop()` する（契約表 3・9）。期限は単調時計（`Instant`）由来のミリ秒だけで測る。
//! 差分: [`ArSample::AwaitingPermission`]（権限ダイアログ待ち）は期限に数えず、起点を捨てて待つ。
//!
//! 映像は `Reading` を得た読み取りでだけ `read_frame` で読み、検査して同じ世代の `apply_frame` で送る。
//! 背景では映像を消す。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS は `ios_ar.rs`（ARKit、Issue #308。
//! 非対応端末は `Unsupported`）、Web は `fandhe-web-dom` 側（(c)）、他 OS は対象外で `UnsupportedAr` 固定。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fandhe_core::screens::ar_show::{
    apply_failure, apply_frame, apply_reading, reading_lines, ArFailureView, ArReadingView,
    ArShowState, ArView, LimitedReasonView, TrackingView,
};
use fandhe_core::RgbaPixels;
#[cfg(not(target_os = "ios"))]
use fandhe_device::ar::UnsupportedAr;
use fandhe_device::ar::{
    ArFailure, ArReading, ArSample, ArTracking, LimitedReason, Tracking, READ_INTERVAL,
};
use fandhe_device::camera_preview_stream::{ScaledFrame, PREVIEW_MAX_SIDE};
use fandhe_device::compass::WaitTracker;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す（プロセスで 1 個。セッションを二重に作らないため
/// `OnceLock` で保持する）。
fn implementation() -> Arc<dyn ArTracking> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn ArTracking>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "ios")]
            {
                super::ios_ar::implementation()
            }
            #[cfg(not(target_os = "ios"))]
            {
                Arc::new(UnsupportedAr)
            }
        })
        .clone()
}

fn to_view(r: ArReading) -> ArReadingView {
    ArReadingView {
        tracking: match r.tracking {
            Tracking::NotAvailable => TrackingView::NotAvailable,
            Tracking::Normal => TrackingView::Normal,
            Tracking::Limited(why) => TrackingView::Limited(match why {
                LimitedReason::Initializing => LimitedReasonView::Initializing,
                LimitedReason::ExcessiveMotion => LimitedReasonView::ExcessiveMotion,
                LimitedReason::InsufficientFeatures => LimitedReasonView::InsufficientFeatures,
                LimitedReason::Relocalizing => LimitedReasonView::Relocalizing,
                LimitedReason::Other => LimitedReasonView::Other,
            }),
        },
        position: r.position,
        horizontal_planes: r.horizontal_planes,
    }
}

/// 契約の失敗を画面の入力へ写す。OS 固有値は画面へ漏らさない（security.md A09）。
fn to_failure(f: ArFailure) -> ArFailureView {
    match f {
        ArFailure::Unsupported => ArFailureView::Unsupported,
        ArFailure::PermissionDenied => ArFailureView::PermissionDenied,
        ArFailure::Failed => ArFailureView::Failed,
    }
}

/// OS 由来の映像を検査して画面の画素にする。寸法・長さが外れたら `None`（映像なし）。
fn to_pixels(f: ScaledFrame) -> Option<RgbaPixels> {
    if f.width.max(f.height) > PREVIEW_MAX_SIDE {
        return None;
    }
    RgbaPixels::new(f.width, f.height, f.rgba)
}

/// driver の記憶（プロセス全体で 1 つ）。
#[derive(Default)]
struct DriverState {
    /// OS のセッションを開始済みの世代。
    started_session: Option<u64>,
    /// 失敗を送信済みの世代（画面側が反映するまで再 `start` しない）。
    failed_session: Option<u64>,
    /// セッションを始めた時刻（経過ミリ秒の基準）。
    epoch: Option<Instant>,
    last_read: Option<Instant>,
    /// 反映済みの復帰回数。
    resume_seen: u64,
    /// 背景中は読み取りも待機の計測も止める（復帰で起点を引き直す）。
    background: bool,
    tracker: WaitTracker,
}

static DRIVER: Mutex<Option<DriverState>> = Mutex::new(None);

fn send_failure(sender: &CompletionSender<ArShowState>, session: u64, f: ArFailureView) {
    let _ = sender.send(Box::new(move |s: &mut ArShowState| {
        apply_failure(s, session, f);
    }));
}

/// 背景からの復帰回数。プラットフォームが復帰イベントで増やし、ドライバは増えたときだけ
/// 待機の起点を引き直す。
static RESUME_EPOCH: AtomicU64 = AtomicU64::new(0);

/// 背景中か。背景中は `NoReading` の待機を進めない。
static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// 背景遷移を記録する。`ios_ar.rs` の受け口（`DidEnterBackground` 由来）が呼ぶ。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub(crate) fn note_backgrounded() {
    BACKGROUND.store(true, Ordering::SeqCst);
}

/// 背景からの復帰を記録する。`ios_ar.rs` の前景復帰の受け口が呼ぶ。復帰回数を先に増やしてから背景フラグを下ろす。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub(crate) fn note_resumed() {
    RESUME_EPOCH.fetch_add(1, Ordering::SeqCst);
    BACKGROUND.store(false, Ordering::SeqCst);
}

/// 開始・停止・間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &ArShowState,
    sender: &CompletionSender<ArShowState>,
    ar: &dyn ArTracking,
    now: Instant,
    resume_epoch: u64,
    ds: &mut DriverState,
) {
    if !state.wanted {
        // 立ち下がり: セッションを止める。待機では再描画を予約しない。
        if ds.started_session.take().is_some() {
            ar.stop();
        }
        ds.failed_session = None;
        ds.epoch = None;
        ds.last_read = None;
        ds.tracker.reset();
        return;
    }
    let session = state.session;
    if ds.failed_session == Some(session) {
        return;
    }
    if ds.background {
        // 契約: 背景では映像を捨てる（復帰直後に古い映像を出さない）。
        if state.frame.is_some() {
            let _ = sender.send(Box::new(move |s: &mut ArShowState| {
                apply_frame(s, session, None);
            }));
        }
        return;
    }
    if ds.started_session != Some(session) {
        // 立ち上がり（または世代交代）: 古いセッションを先に止めてから開始する。
        if ds.started_session.take().is_some() {
            ar.stop();
        }
        if let Err(e) = ar.start() {
            ar.stop();
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

    // 復帰の記録は起点を引き直す読み取りまで消費しない（間隔待ちの early return で失わない）。
    let resumed = ds.resume_seen != resume_epoch;
    // 背景中に drive が走らず映像を捨てそびれた場合の保険。復帰を観測したら間隔判定より前に一度捨てる。
    if resumed && state.frame.is_some() {
        let _ = sender.send(Box::new(move |s: &mut ArShowState| {
            apply_frame(s, session, None);
        }));
    }
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
    let mut frame: Option<RgbaPixels> = None;
    let next: Option<ArReadingView> = match ar.read() {
        Ok(ArSample::Reading(r)) => {
            ds.tracker.observe(true, now_ms);
            frame = ar.read_frame().and_then(to_pixels);
            Some(to_view(r))
        }
        Ok(ArSample::AwaitingPermission) => {
            // 権限ダイアログ待ちは期限に数えない。応答後の最初の `Waiting` が新しい起点になる。
            ds.tracker.reset();
            None
        }
        Ok(ArSample::Waiting) => {
            if ds.tracker.observe(false, now_ms) {
                ar.stop();
                ds.started_session = None;
                ds.failed_session = Some(session);
                send_failure(sender, session, ArFailureView::NoReading);
                return;
            }
            None
        }
        Err(e) => {
            ar.stop();
            ds.started_session = None;
            ds.failed_session = Some(session);
            send_failure(sender, session, to_failure(e));
            return;
        }
    };

    // 表示 3 行が同じなら送らない。
    let shown = match (&state.view, &next) {
        (ArView::Live(cur), Some(n)) => reading_lines(cur) == reading_lines(n),
        (ArView::Starting, None) => true,
        _ => false,
    };
    if !shown {
        let _ = sender.send(Box::new(move |s: &mut ArShowState| {
            apply_reading(s, session, next);
        }));
    }
    // 映像は 3 行とは別に送る。`apply_reading` の後に積むので Live の後に届く。
    // ponytail: 毎回 Arc を確保して送る（10 fps・長辺 320 なら許容）。負荷が出たら差分送信へ。
    if frame.is_some() || state.frame.is_some() {
        let _ = sender.send(Box::new(move |s: &mut ArShowState| {
            apply_frame(s, session, frame);
        }));
    }
}

/// `registry.rs` の `effect` 区画から `ar_show` 画面用に配線される effect ドライバ。
pub fn ar_show_effect(state: &ArShowState, sender: &CompletionSender<ArShowState>) {
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
    use fandhe_core::screens::ar_show::{main_text, start, stop};
    use fandhe_device::ar::UnsupportedAr as Unsupported;
    use fandhe_device::compass::NO_READING_TIMEOUT_MS;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<ArShowState>>;

    fn channel() -> (CompletionSender<ArShowState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply_all(rx: &Rx, s: &mut ArShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    struct Fake {
        start: Result<(), ArFailure>,
        read: StdMutex<Result<ArSample, ArFailure>>,
        starts: AtomicUsize,
        reads: AtomicUsize,
        frame: StdMutex<Option<ScaledFrame>>,
        frame_reads: AtomicUsize,
        stops: AtomicUsize,
    }

    impl Fake {
        fn new(read: Result<ArSample, ArFailure>) -> Self {
            Self {
                start: Ok(()),
                read: StdMutex::new(read),
                starts: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
                frame: StdMutex::new(None),
                frame_reads: AtomicUsize::new(0),
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

    impl ArTracking for Fake {
        fn start(&self) -> Result<(), ArFailure> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.start
        }
        fn read(&self) -> Result<ArSample, ArFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            *self.read.lock().unwrap()
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
        fn read_frame(&self) -> Option<ScaledFrame> {
            self.frame_reads.fetch_add(1, Ordering::SeqCst);
            self.frame.lock().unwrap().clone()
        }
    }

    fn sf(w: u32, h: u32, len: usize) -> ScaledFrame {
        ScaledFrame {
            width: w,
            height: h,
            rgba: vec![0; len],
        }
    }

    fn set_frame(fake: &Fake, f: Option<ScaledFrame>) {
        *fake.frame.lock().unwrap() = f;
    }

    fn reading(x: f64) -> ArSample {
        ArSample::Reading(ArReading {
            tracking: Tracking::Normal,
            position: Some([x, 0.0, 0.0]),
            horizontal_planes: Some(1),
        })
    }

    fn wanted() -> ArShowState {
        let mut s = ArShowState::default();
        start(&mut s);
        s
    }

    fn at(t0: Instant, ms: f64) -> Instant {
        t0 + Duration::from_secs_f64(ms / 1000.0)
    }

    #[test]
    fn start_once_stop_once_and_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.5)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        assert_eq!(fake.counts(), (1, 2, 0));
        assert_eq!(main_text(&s), "追跡: 良好");
        stop(&mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 3, 0, &mut ds);
        assert_eq!(fake.counts(), (1, 2, 1));
    }

    #[test]
    fn idle_never_reads_or_starts() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.0)));
        let state = ArShowState::default();
        drive(
            &state,
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
            ArFailure::Unsupported,
            ArFailure::PermissionDenied,
            ArFailure::Failed,
        ] {
            let (tx, rx) = channel();
            let mut fake = Fake::new(Ok(reading(0.0)));
            fake.start = Err(f);
            let mut s = wanted();
            let mut ds = DriverState::default();
            let t0 = Instant::now();
            drive(&s, &tx, &fake, t0, 0, &mut ds);
            drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
            apply_all(&rx, &mut s);
            assert!(!s.wanted);
            assert!(matches!(s.view, ArView::Failed(_)));
            assert_eq!(fake.counts(), (1, 0, 1));
        }
    }

    #[test]
    fn read_error_stops_then_fails() {
        let (tx, rx) = channel();
        let fake = Fake::new(Err(ArFailure::PermissionDenied));
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
        assert_eq!(s.view, ArView::Failed(ArFailureView::PermissionDenied));
    }

    #[test]
    fn no_value_for_timeout_stops_then_no_reading() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ArSample::Waiting));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, ArView::Starting);
        let mut ms = 500.0;
        while ms <= NO_READING_TIMEOUT_MS {
            drive(&s, &tx, &fake, at(t0, ms), 0, &mut ds);
            ms += 500.0;
        }
        apply_all(&rx, &mut s);
        assert_eq!((s.view, fake.counts().2), (ArView::Starting, 0));
        drive(
            &s,
            &tx,
            &fake,
            at(t0, NO_READING_TIMEOUT_MS + 200.0),
            0,
            &mut ds,
        );
        apply_all(&rx, &mut s);
        assert_eq!(fake.counts().2, 1);
        assert_eq!(s.view, ArView::Failed(ArFailureView::NoReading));
    }

    #[test]
    fn awaiting_permission_does_not_expire_then_waiting_does() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ArSample::AwaitingPermission));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        let mut ms = 0.0;
        while ms <= 10_000.0 {
            drive(&s, &tx, &fake, at(t0, ms), 0, &mut ds);
            ms += 500.0;
        }
        apply_all(&rx, &mut s);
        assert_eq!((s.view, fake.counts().2), (ArView::Starting, 0));
        // 許可後に Waiting へ変わった時点が新しい起点。そこから 3 秒ちょうどでは失敗しない。
        *fake.read.lock().unwrap() = Ok(ArSample::Waiting);
        drive(&s, &tx, &fake, at(t0, 11_000.0), 0, &mut ds);
        drive(
            &s,
            &tx,
            &fake,
            at(t0, 11_000.0 + NO_READING_TIMEOUT_MS),
            0,
            &mut ds,
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.view, ArView::Starting);
        drive(
            &s,
            &tx,
            &fake,
            at(t0, 11_000.0 + NO_READING_TIMEOUT_MS + 200.0),
            0,
            &mut ds,
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.view, ArView::Failed(ArFailureView::NoReading));
    }

    #[test]
    fn long_gap_without_resume_still_times_out() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ArSample::Waiting));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(10), 0, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, ArView::Failed(ArFailureView::NoReading));
    }

    #[test]
    fn resume_during_interval_is_not_lost() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ArSample::Waiting));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        drive(&s, &tx, &fake, t0 + ms(2900), 0, &mut ds);
        drive(&s, &tx, &fake, t0 + ms(2950), 1, &mut ds);
        drive(&s, &tx, &fake, t0 + ms(3100), 1, &mut ds);
        apply_all(&rx, &mut s);
        assert_eq!((s.view, fake.counts().2), (ArView::Starting, 0));
    }

    #[test]
    fn background_pauses_wait_and_resume_restarts_it() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ArSample::Waiting));
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
        assert_eq!((s.view, fake.counts().2), (ArView::Starting, 0));
    }

    #[test]
    fn same_display_is_not_sent_but_changed_is() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.501)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(50), 0, &mut ds);
        assert_eq!(fake.counts().1, 1);
        // 表示上同じ 0.50 m なら送らない。
        *fake.read.lock().unwrap() = Ok(reading(0.499));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        assert!(rx.try_recv().is_err());
        *fake.read.lock().unwrap() = Ok(reading(0.7));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 2, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert!(matches!(&s.view, ArView::Live(r) if r.position == Some([0.7, 0.0, 0.0])));
    }

    #[test]
    fn stale_session_completion_is_not_live() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(1.0)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        drive(&s, &tx, &fake, Instant::now(), 0, &mut ds);
        stop(&mut s);
        start(&mut s);
        apply_all(&rx, &mut s);
        assert_eq!(s.view, ArView::Starting);
    }

    #[test]
    fn new_session_stops_old_before_start() {
        let (tx, _rx) = channel();
        let fake = Fake::new(Ok(ArSample::Waiting));
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
        assert_eq!(s.view, ArView::Failed(ArFailureView::Unsupported));
    }

    #[test]
    fn frame_sent_only_with_reading() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.5)));
        set_frame(&fake, Some(sf(2, 2, 16)));
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
        assert!(s.frame.is_some());
        assert_eq!(fake.frame_reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn frame_not_read_without_reading() {
        let cases = [
            Ok(ArSample::Waiting),
            Ok(ArSample::AwaitingPermission),
            Err(ArFailure::PermissionDenied),
            Err(ArFailure::Failed),
        ];
        for c in cases {
            let (tx, rx) = channel();
            let fake = Fake::new(c);
            set_frame(&fake, Some(sf(2, 2, 16)));
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
            assert_eq!(fake.frame_reads.load(Ordering::SeqCst), 0);
            assert!(s.frame.is_none());
        }
        // 期限切れ
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(ArSample::Waiting));
        set_frame(&fake, Some(sf(2, 2, 16)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        drive(
            &s,
            &tx,
            &fake,
            at(t0, NO_READING_TIMEOUT_MS + 200.0),
            0,
            &mut ds,
        );
        apply_all(&rx, &mut s);
        assert_eq!(fake.frame_reads.load(Ordering::SeqCst), 0);
        assert!(s.frame.is_none());
    }

    #[test]
    fn invalid_frame_is_no_video_and_clears_previous() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.5)));
        set_frame(&fake, Some(sf(2, 2, 16)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_some());
        for (i, bad) in [sf(321, 1, 321 * 4), sf(2, 2, 15), sf(0, 2, 0)]
            .into_iter()
            .enumerate()
        {
            set_frame(&fake, Some(bad));
            drive(
                &s,
                &tx,
                &fake,
                t0 + READ_INTERVAL * (i as u32 * 2 + 1),
                0,
                &mut ds,
            );
            apply_all(&rx, &mut s);
            assert!(s.frame.is_none());
            set_frame(&fake, Some(sf(2, 2, 16)));
            drive(
                &s,
                &tx,
                &fake,
                t0 + READ_INTERVAL * (i as u32 * 2 + 2),
                0,
                &mut ds,
            );
            apply_all(&rx, &mut s);
            assert!(s.frame.is_some());
        }
    }

    #[test]
    fn frame_updates_even_if_lines_unchanged() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.5)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_none());
        set_frame(&fake, Some(sf(1, 1, 4)));
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_some());
    }

    #[test]
    fn frame_cleared_when_reading_has_no_frame() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.5)));
        set_frame(&fake, Some(sf(1, 1, 4)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_some());
        set_frame(&fake, None);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_none());
    }

    #[test]
    fn stale_session_frame_is_dropped() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.5)));
        set_frame(&fake, Some(sf(1, 1, 4)));
        let mut s = wanted();
        drive(
            &s,
            &tx,
            &fake,
            Instant::now(),
            0,
            &mut DriverState::default(),
        );
        stop(&mut s);
        start(&mut s);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_none());
    }

    #[test]
    fn background_clears_frame() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.5)));
        set_frame(&fake, Some(sf(1, 1, 4)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_some());
        ds.background = true;
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_none());
    }

    #[test]
    fn resume_without_background_drive_clears_stale_frame() {
        let (tx, rx) = channel();
        let fake = Fake::new(Ok(reading(0.5)));
        set_frame(&fake, Some(sf(1, 1, 4)));
        let mut s = wanted();
        let mut ds = DriverState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, 0, &mut ds);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_some());
        // 背景中は drive が走らず、復帰後の最初の drive が間隔待ちでも古い映像を捨てる。
        set_frame(&fake, None);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(50), 1, &mut ds);
        apply_all(&rx, &mut s);
        assert!(s.frame.is_none());
    }

    #[test]
    fn unsupported_impl_never_shows_frame() {
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
        assert!(s.frame.is_none());
    }
}
