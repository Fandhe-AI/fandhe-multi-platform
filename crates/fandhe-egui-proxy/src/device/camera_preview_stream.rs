//! Phase 11 の拡充ループ（親 #168、Issue #812）型サンプル「カメラのライブ映像」の effect ドライバ。
//! `fandhe_core::screens::camera_preview_stream_show`（端末依存ゼロ）の要求を `take_start` /
//! `take_zoom` で観測し、`fandhe_device::camera_preview_stream::CameraPreview` を呼んで、結果を
//! `CompletionSender` 経由で注入する（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は
//! `fandhe_device::camera_preview_stream`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_camera_preview`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（iOS: `ios_camera_preview_stream`（#813）、Android:
//! `android_camera_preview_stream`（#814））。macOS・Windows は保留で、Linux 等と同じく
//! `UnsupportedCameraPreview` 固定（ダミーの成功実装へは倒さない）。Web は本ドライバを通らない
//! （#815 の `fandhe-web-dom`）。OS のコールバックは [`register`] で得た `id` を持ち、
//! [`deliver_frame`] / [`deliver_event`] を呼ぶ。
//!
//! # 方式と並行性（契約表 11）
//!
//! OS から届いたフレームは受信口（[`Inbox`]）に最新 1 枚だけ残し、通知は 1 本の列に積む。
//! 空の完了（wake）は `woken` が下りていたときだけ 1 件送る（100 回届いても 1 件）。
//! 状態の更新・開始・停止・ズームは、すべて UI スレッドのこの 1 本の effect に集約する。
//! OS コールバックが触るのは受信口と wake だけ。共通コアは `fandhe-device` の型を持てず effect は
//! `&S` しか受け取れないため、セッションは [`ACTIVE`] に置く。解放は [`Active`] の drop で、
//! ロックの外で行う（登録解除 → セッション drop）。
//!
//! # 嘘の成功の禁止・プライバシー
//!
//! 非対応・失敗を成功へ畳まない。ログ・エラー文言に OS のメッセージ・画素を出さない
//! （security.md A09）。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use fandhe_core::screens::camera_preview_stream_show::{
    apply_frame, apply_interrupted, apply_running, apply_runtime_failed, apply_start_failed,
    apply_stopped, apply_unsupported, apply_zoom_failed, finish_zoom, take_start, take_zoom,
    CameraPreviewStreamShowState, FailureView, FrameView, PreviewPhase, StopView, ZoomFailureView,
    ZoomStepView,
};
use fandhe_core::RgbaPixels;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_device::camera_preview_stream::UnsupportedCameraPreview;
use fandhe_device::camera_preview_stream::{
    checked_zoom, CameraPreview, PreviewEvent, PreviewFrame, PreviewSession, PreviewSink,
    PreviewStartError, StartFailure, StopReason, ZoomFailure, ZoomStep, PREVIEW_MAX_SIDE,
};

use crate::app::CompletionSender;

type Sender = CompletionSender<CameraPreviewStreamShowState>;

/// 受信列の上限。あふれたら古い非終端イベントから捨て、終端イベントを優先して残す。
/// 仮置きの値で、実機確認で見直す。
const MAX_PENDING_EVENTS: usize = 32;

/// 現在のプラットフォームに応じた `CameraPreview` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_camera_preview() -> Arc<dyn CameraPreview> {
    #[cfg(target_os = "ios")]
    {
        super::ios_camera_preview_stream::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_camera_preview_stream::implementation()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        Arc::new(UnsupportedCameraPreview)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- 登録表（全 OS 共通）-------------------------------------------------------------------

type Sinks = Mutex<HashMap<u64, Arc<dyn PreviewSink>>>;
static SINKS: std::sync::LazyLock<Sinks> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// OS コールバックが引く `id` を発行して受け口を登録する。OS 側は生ポインタではなくこの `id` だけを
/// 持ち、停止後に遅れて届いた呼び出しは [`deliver_frame`] / [`deliver_event`] が捨てる。
pub(crate) fn register(sink: Arc<dyn PreviewSink>) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    lock(&SINKS).insert(id, sink);
    id
}

/// 登録を外す。停止の最初に呼び、以後の配信は何もしない。
pub(crate) fn unregister(id: u64) {
    lock(&SINKS).remove(&id);
}

#[allow(dead_code)] // deliver_* 経由で #813・#814 が使い始めるまで未使用。
fn lookup(id: u64) -> Option<Arc<dyn PreviewSink>> {
    lock(&SINKS).get(&id).cloned()
}

/// フレームを届ける。ロックの内側では sink を呼ばず、登録表に無い `id`（停止後の遅れた呼び出し）は捨てる。
/// iOS・Android の OS 実装（#813・#814）の受け口から呼ばれる。
#[allow(dead_code)] // #813・#814 が使い始めるまで未使用。
pub(crate) fn deliver_frame(id: u64, frame: PreviewFrame) {
    if let Some(sink) = lookup(id) {
        sink.frame(frame);
    }
}

/// 通知を届ける。扱いは [`deliver_frame`] と同じ。
#[allow(dead_code)] // #813・#814 が使い始めるまで未使用。
pub(crate) fn deliver_event(id: u64, event: PreviewEvent) {
    if let Some(sink) = lookup(id) {
        sink.event(event);
    }
}

// ---- 受信口 --------------------------------------------------------------------------------

/// 検証済みの最新フレーム。
struct ReadyFrame {
    pixels: RgbaPixels,
    source_width: u32,
    source_height: u32,
    zoom: Option<f32>,
}

/// セッションごとの受信口。OS スレッドが書き、UI スレッドの effect が読む。
#[derive(Default)]
struct Inbox {
    /// 最新の 1 枚（古いものは上書きして捨てる）。
    latest: Option<ReadyFrame>,
    /// 検証を通って届いたフレームの総数。
    received: u64,
    /// 通知の受信列（1 本）。
    events: VecDeque<PreviewEvent>,
}

#[derive(Default)]
struct Shared {
    inbox: Mutex<Inbox>,
    /// 空の完了を送ってまだ排出していないか。
    woken: AtomicBool,
    /// 解放済みか。解放後に OS が直接 sink を呼んでも何もしない。
    closed: AtomicBool,
}

/// OS へ渡す受け口。受信口へ入れて wake を 1 件だけ送る。
struct QueueSink {
    shared: Arc<Shared>,
    sender: Sender,
}

impl QueueSink {
    fn wake(&self) {
        if !self.shared.woken.swap(true, Ordering::SeqCst) {
            // 送信失敗（終了中）は無視してよい。
            let _ = self
                .sender
                .send(Box::new(|_: &mut CameraPreviewStreamShowState| {}));
            // iOS・Android は入力が無いと egui が描画しないため、次フレームを起こす（契約表 4）。
            #[cfg(target_os = "ios")]
            crate::ios::request_repaint();
            #[cfg(target_os = "android")]
            crate::android::request_repaint();
        }
    }
}

impl PreviewSink for QueueSink {
    fn frame(&self, f: PreviewFrame) {
        if self.shared.closed.load(Ordering::SeqCst) {
            return;
        }
        // 縮小後の寸法・長さを検証する。不正なものは捨て、数えない。
        let (fw, fh) = (f.frame.width, f.frame.height);
        if fw.max(fh) > PREVIEW_MAX_SIDE {
            return;
        }
        let Some(pixels) = RgbaPixels::new(fw, fh, f.frame.rgba) else {
            return;
        };
        {
            let mut ib = lock(&self.shared.inbox);
            ib.latest = Some(ReadyFrame {
                pixels,
                source_width: f.source_width,
                source_height: f.source_height,
                zoom: f.zoom.and_then(checked_zoom),
            });
            ib.received += 1;
        }
        self.wake();
    }

    fn event(&self, e: PreviewEvent) {
        if self.shared.closed.load(Ordering::SeqCst) {
            return;
        }
        {
            let mut ib = lock(&self.shared.inbox);
            // 中断前に届いたフレームは、再開後の映像として適用しない。通知列と最新フレームは別々に
            // 保持するため到着順が失われる。そこで中断の通知を受けた同じロックの内側で破棄する。
            if matches!(e, PreviewEvent::Interrupted) {
                ib.latest = None;
            }
            if ib.events.len() >= MAX_PENDING_EVENTS {
                if let Some(i) = ib.events.iter().position(|x| !x.is_terminal()) {
                    ib.events.remove(i);
                } else if e.is_terminal() {
                    ib.events.pop_front();
                } else {
                    return;
                }
            }
            ib.events.push_back(e);
        }
        self.wake();
    }
}

// ---- ドライバ ------------------------------------------------------------------------------

/// 実行中の映像 1 件。drop で登録解除してから OS のセッションを止める。
struct Active {
    /// 開始要求の世代。
    generation: u64,
    /// 登録表の `id`。
    id: u64,
    shared: Arc<Shared>,
    /// 最後に処理したズーム要求の通し番号。
    last_zoom_seq: u64,
    /// drop で OS の映像を止める（`Active::drop` の後に落ちる）。
    session: Box<dyn PreviewSession>,
}

impl Drop for Active {
    fn drop(&mut self) {
        unregister(self.id);
        self.shared.closed.store(true, Ordering::SeqCst);
    }
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// プロセス終了時の解放（`device::shutdown` から）。デスクトップやモバイルでは「画面を離れる」と
/// プロセスが終わるため、ここが解放の経路になる。停止の待ちはロックの外で行う。
pub(crate) fn shutdown() {
    let a = lock(&ACTIVE).take();
    drop(a);
}

/// Activity 再生成時（`android_main` の再実行）に、実行中の映像を止めて破棄する。画面状態は再生成で
/// `default()` に戻るため、残すと旧セッションが動いたまま新しい UI スレッドと [`ACTIVE`] を取り合う。
/// 他の長時間ドライバの `reset_pending_tracking` と同じ位置づけで、停止の待ちはロックの外で行う。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    shutdown();
}

fn to_failure_view(f: StartFailure) -> FailureView {
    match f {
        StartFailure::PermissionDenied => FailureView::PermissionDenied,
        StartFailure::NoCamera => FailureView::NoCamera,
        StartFailure::InUse => FailureView::InUse,
        StartFailure::Other => FailureView::Other,
    }
}

fn to_stop_view(r: StopReason) -> StopView {
    match r {
        StopReason::Backgrounded => StopView::Backgrounded,
        StopReason::LeftScreen => StopView::LeftScreen,
    }
}

fn to_zoom_failure_view(z: ZoomFailure) -> ZoomFailureView {
    match z {
        ZoomFailure::Unsupported => ZoomFailureView::Unsupported,
        ZoomFailure::Failed => ZoomFailureView::Failed,
    }
}

fn to_zoom_step(s: ZoomStepView) -> ZoomStep {
    match s {
        ZoomStepView::One => ZoomStep::One,
        ZoomStepView::Two => ZoomStep::Two,
    }
}

/// 通知 1 件を画面の状態へ写して送る。
fn send_event(sender: &Sender, generation: u64, e: PreviewEvent) {
    let _ = sender.send(Box::new(
        move |s: &mut CameraPreviewStreamShowState| match e {
            PreviewEvent::Running => apply_running(s, generation),
            PreviewEvent::Interrupted => apply_interrupted(s, generation),
            PreviewEvent::Stopped(r) => apply_stopped(s, generation, to_stop_view(r)),
            PreviewEvent::StartFailed(f) => apply_start_failed(s, generation, to_failure_view(f)),
            PreviewEvent::RuntimeFailed => apply_runtime_failed(s, generation),
            PreviewEvent::ZoomFailed(z) => {
                apply_zoom_failed(s, generation, to_zoom_failure_view(z))
            }
        },
    ));
}

/// 毎フレーム 1 回、UI スレッドで同期に処理する（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &CameraPreviewStreamShowState,
    sender: &Sender,
    active: &Mutex<Option<Active>>,
    preview: &dyn CameraPreview,
) {
    // 1. 照合: 画面が映像を求めていない / 世代が違う映像は止める。利用者の停止、Android の
    //    Activity 再生成後の初期 state、古い世代をここ 1 か所で吸収する。停止の待ちはロックの外。
    let stale = {
        let mut g = lock(active);
        let keep = g.as_ref().is_some_and(|a| {
            a.generation == state.generation
                && matches!(
                    state.phase,
                    PreviewPhase::Starting | PreviewPhase::Live | PreviewPhase::Interrupted
                )
        });
        if keep {
            None
        } else {
            g.take()
        }
    };
    drop(stale);

    // 2. 開始（二重に起こさない）。許可待ち・open の途中でも session はすぐ返り、drop で取り消せる。
    if let Some(generation) = take_start(state) {
        if lock(active).is_none() {
            let shared = Arc::new(Shared::default());
            let sink = Arc::new(QueueSink {
                shared: shared.clone(),
                sender: sender.clone(),
            });
            let id = register(sink.clone());
            match preview.start(sink) {
                Ok(session) => {
                    *lock(active) = Some(Active {
                        generation,
                        id,
                        shared,
                        last_zoom_seq: 0,
                        session,
                    });
                }
                Err(e) => {
                    unregister(id);
                    shared.closed.store(true, Ordering::SeqCst);
                    let _ = sender.send(Box::new(
                        move |s: &mut CameraPreviewStreamShowState| match e {
                            PreviewStartError::Unsupported => apply_unsupported(s, generation),
                            PreviewStartError::Failed(f) => {
                                apply_start_failed(s, generation, to_failure_view(f))
                            }
                        },
                    ));
                }
            }
        }
    }

    // 3. ズーム。同じ要求では 1 回しか呼ばない。`set_zoom` は契約上すぐ戻る。
    if let Some(req) = take_zoom(state) {
        let result = {
            let mut g = lock(active);
            match g.as_mut() {
                Some(a) if a.generation == req.generation && a.last_zoom_seq != req.seq => {
                    a.last_zoom_seq = req.seq;
                    Some(a.session.set_zoom(to_zoom_step(req.step)))
                }
                _ => None,
            }
        };
        if let Some(r) = result {
            let r = r.map_err(to_zoom_failure_view);
            let _ = sender.send(Box::new(move |s: &mut CameraPreviewStreamShowState| {
                finish_zoom(s, req.generation, req.seq, r)
            }));
        }
    }

    // 4. 排出。`woken` を先に下ろし、排出中に届いた分は次のフレームを起こす。
    let snapshot = lock(active)
        .as_ref()
        .map(|a| (a.shared.clone(), a.generation));
    let Some((shared, generation)) = snapshot else {
        return;
    };
    shared.woken.store(false, Ordering::SeqCst);
    let (events, latest, received) = {
        let mut ib = lock(&shared.inbox);
        (
            std::mem::take(&mut ib.events),
            ib.latest.take(),
            ib.received,
        )
    };
    let mut terminal = false;
    for e in events {
        send_event(sender, generation, e);
        if e.is_terminal() {
            terminal = true;
            break; // 終端以降の通知・フレームは捨てる。
        }
    }
    if terminal {
        let a = lock(active).take();
        drop(a);
        return;
    }
    if let Some(f) = latest {
        let view = FrameView {
            pixels: f.pixels,
            source_width: f.source_width,
            source_height: f.source_height,
        };
        let zoom = f.zoom;
        let _ = sender.send(Box::new(move |s: &mut CameraPreviewStreamShowState| {
            apply_frame(s, generation, received, view, zoom)
        }));
    }
}

/// `registry.rs` の `effect` 区画から `camera_preview_stream_show` 画面用に配線される effect ドライバ。
pub fn camera_preview_stream_show_effect(state: &CameraPreviewStreamShowState, sender: &Sender) {
    drive(state, sender, &ACTIVE, platform_camera_preview().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::camera_preview_stream_show::{
        start, status_text, stop, zoom_two, ZoomView, FAILED_PREFIX, PERMISSION_DENIED_REASON,
    };
    use fandhe_device::camera_preview_stream::{ScaledFrame, UnsupportedCameraPreview};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    struct FakeSession {
        drops: Arc<AtomicUsize>,
        zooms: Arc<AtomicUsize>,
    }
    impl PreviewSession for FakeSession {
        fn set_zoom(&self, _: ZoomStep) -> Result<(), ZoomFailure> {
            self.zooms.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    impl Drop for FakeSession {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct Fake {
        fail: Option<PreviewStartError>,
        sink: Mutex<Option<Arc<dyn PreviewSink>>>,
        drops: Arc<AtomicUsize>,
        zooms: Arc<AtomicUsize>,
        starts: AtomicUsize,
    }
    impl Fake {
        fn sink(&self) -> Arc<dyn PreviewSink> {
            lock(&self.sink).clone().expect("start 済み")
        }
    }
    impl CameraPreview for Fake {
        fn start(
            &self,
            sink: Arc<dyn PreviewSink>,
        ) -> Result<Box<dyn PreviewSession>, PreviewStartError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = self.fail {
                return Err(e);
            }
            *lock(&self.sink) = Some(sink);
            Ok(Box::new(FakeSession {
                drops: self.drops.clone(),
                zooms: self.zooms.clone(),
            }))
        }
    }

    struct NopSink;
    impl PreviewSink for NopSink {
        fn frame(&self, _: PreviewFrame) {}
        fn event(&self, _: PreviewEvent) {}
    }

    struct Env {
        active: Mutex<Option<Active>>,
        fake: Fake,
        state: CameraPreviewStreamShowState,
        sender: Sender,
        rx: mpsc::Receiver<crate::app::Completion<CameraPreviewStreamShowState>>,
    }
    impl Env {
        fn new(fake: Fake) -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                active: Mutex::new(None),
                fake,
                state: CameraPreviewStreamShowState::default(),
                sender: CompletionSender::new(tx, egui::Context::default()),
                rx,
            }
        }
        fn apply_queued(&mut self) {
            while let Ok(c) = self.rx.try_recv() {
                c(&mut self.state);
            }
        }
        fn tick(&mut self) {
            drive(&self.state, &self.sender, &self.active, &self.fake);
            self.apply_queued();
        }
        /// 開始して Running まで進める。
        fn live() -> Self {
            let mut e = Self::new(Fake::default());
            start(&mut e.state);
            e.tick();
            e.fake.sink().event(PreviewEvent::Running);
            e.tick();
            assert_eq!(e.state.phase, PreviewPhase::Live);
            e
        }
        fn drops(&self) -> usize {
            self.fake.drops.load(Ordering::SeqCst)
        }
        fn active_id(&self) -> Option<u64> {
            lock(&self.active).as_ref().map(|a| a.id)
        }
    }

    fn frame(zoom: Option<f32>) -> PreviewFrame {
        PreviewFrame {
            source_width: 1280,
            source_height: 720,
            frame: ScaledFrame {
                width: 2,
                height: 1,
                rgba: vec![9; 8],
            },
            zoom,
        }
    }

    #[test]
    fn start_error_leaves_nothing_registered() {
        let mut e = Env::new(Fake {
            fail: Some(PreviewStartError::Failed(StartFailure::PermissionDenied)),
            ..Fake::default()
        });
        start(&mut e.state);
        e.tick();
        assert!(e.active_id().is_none());
        // 失敗した開始の sink は登録表に残らない（Fake は sink を保持しない）。
        assert!(lock(&e.fake.sink).is_none());
        assert_eq!(
            status_text(&e.state),
            format!("{FAILED_PREFIX}{PERMISSION_DENIED_REASON}")
        );
    }

    #[test]
    fn unsupported_start_is_not_success() {
        let mut e = Env::new(Fake {
            fail: Some(PreviewStartError::Unsupported),
            ..Fake::default()
        });
        start(&mut e.state);
        e.tick();
        assert_eq!(
            e.state.phase,
            PreviewPhase::Failed(FailureView::Unsupported)
        );
    }

    #[test]
    fn host_dispatch_never_succeeds() {
        let mut e = Env::new(Fake::default());
        start(&mut e.state);
        drive(&e.state, &e.sender, &e.active, &UnsupportedCameraPreview);
        e.apply_queued();
        assert_eq!(
            e.state.phase,
            PreviewPhase::Failed(FailureView::Unsupported)
        );
        // 実際のホスト dispatch も同じ（iOS・Android 以外は Unsupported 固定）。
        assert!(matches!(
            platform_camera_preview().start(Arc::new(NopSink)),
            Err(PreviewStartError::Unsupported)
        ));
    }

    #[test]
    fn stop_while_starting_drops_session() {
        let mut e = Env::new(Fake::default());
        start(&mut e.state);
        e.tick();
        assert!(e.active_id().is_some());
        assert_eq!(e.state.phase, PreviewPhase::Starting);
        stop(&mut e.state);
        e.tick();
        assert_eq!(e.drops(), 1);
        assert!(e.active_id().is_none());
    }

    #[test]
    fn permission_denied_event_fails_and_releases() {
        let mut e = Env::new(Fake::default());
        start(&mut e.state);
        e.tick();
        e.fake
            .sink()
            .event(PreviewEvent::StartFailed(StartFailure::PermissionDenied));
        e.tick();
        assert_eq!(
            e.state.phase,
            PreviewPhase::Failed(FailureView::PermissionDenied)
        );
        assert_eq!(e.drops(), 1);
    }

    #[test]
    fn late_delivery_after_stop_is_dropped() {
        let mut e = Env::live();
        let id = e.active_id().unwrap();
        stop(&mut e.state);
        e.tick();
        assert_eq!(e.drops(), 1);
        assert!(lookup(id).is_none(), "登録解除済み");
        deliver_frame(id, frame(Some(1.0)));
        deliver_event(id, PreviewEvent::RuntimeFailed);
        // 直接 sink を握っている OS 側が呼んでも何も起きない。
        e.fake.sink().frame(frame(Some(1.0)));
        e.fake.sink().event(PreviewEvent::RuntimeFailed);
        e.tick();
        assert_eq!(e.state.phase, PreviewPhase::Stopped(StopView::User));
        assert!(e.state.frame.is_none());
    }

    #[test]
    fn stale_generation_event_does_not_change_state() {
        let mut e = Env::live();
        let g = e.state.generation;
        send_event(&e.sender, g + 5, PreviewEvent::RuntimeFailed);
        e.apply_queued();
        assert_eq!(e.state.phase, PreviewPhase::Live);
    }

    #[test]
    fn terminal_event_releases_session_and_drops_later_frames() {
        let mut e = Env::live();
        e.fake.sink().frame(frame(Some(1.0)));
        e.fake.sink().event(PreviewEvent::RuntimeFailed);
        e.fake.sink().event(PreviewEvent::Running);
        e.tick();
        assert_eq!(e.drops(), 1);
        assert_eq!(e.state.phase, PreviewPhase::Failed(FailureView::Runtime));
        assert!(e.state.frame.is_none());
    }

    #[test]
    fn many_frames_wake_once_and_keep_latest() {
        let mut e = Env::live();
        let sink = e.fake.sink();
        for i in 0..100u32 {
            let mut f = frame(Some(1.0));
            f.source_width = 1000 + i;
            sink.frame(f);
        }
        assert_eq!(e.rx.try_iter().count(), 1, "wake は 1 件");
        e.tick();
        assert_eq!(e.state.frames_received, 100);
        assert_eq!(e.state.frame.as_ref().unwrap().source_width, 1099);
        assert_eq!(e.state.zoom, ZoomView::Value(1.0));
    }

    #[test]
    fn invalid_frames_are_dropped_and_not_counted() {
        let mut e = Env::live();
        let sink = e.fake.sink();
        let mut bad = frame(None);
        bad.frame.rgba = vec![0; 3];
        sink.frame(bad);
        let mut big = frame(None);
        big.frame = ScaledFrame {
            width: PREVIEW_MAX_SIDE + 1,
            height: 1,
            rgba: vec![0; (PREVIEW_MAX_SIDE as usize + 1) * 4],
        };
        sink.frame(big);
        e.tick();
        assert_eq!(e.state.frames_received, 0);
        assert!(e.state.frame.is_none());
        // 不正なズーム値は「読めない」になる。
        sink.frame(frame(Some(f32::NAN)));
        e.tick();
        assert_eq!(e.state.zoom, ZoomView::Unavailable);
    }

    #[test]
    fn shutdown_drops_session() {
        let e = Env::live();
        // `shutdown` と同じ解放経路（スロットから take して drop）を、ローカルのスロットで確かめる。
        let a = lock(&e.active).take();
        drop(a);
        assert_eq!(e.drops(), 1);
        // 静的な ACTIVE が空でも安全。
        shutdown();
    }

    #[test]
    fn same_zoom_request_calls_set_zoom_once() {
        let mut e = Env::live();
        zoom_two(&mut e.state);
        // 完了が届く前の再実行でも 1 回。
        drive(&e.state, &e.sender, &e.active, &e.fake);
        drive(&e.state, &e.sender, &e.active, &e.fake);
        assert_eq!(e.fake.zooms.load(Ordering::SeqCst), 1);
        e.apply_queued();
        assert!(e.state.zoom_request.is_none());
    }

    #[test]
    fn overflowing_events_keep_terminal() {
        let mut e = Env::live();
        let sink = e.fake.sink();
        for _ in 0..(MAX_PENDING_EVENTS * 2) {
            sink.event(PreviewEvent::Interrupted);
        }
        sink.event(PreviewEvent::RuntimeFailed);
        e.tick();
        assert_eq!(e.state.phase, PreviewPhase::Failed(FailureView::Runtime));
        assert_eq!(e.drops(), 1);
    }

    #[test]
    fn frame_before_interruption_is_not_applied_after_resume() {
        let mut e = Env::live();
        let sink = e.fake.sink();
        sink.frame(frame(Some(2.0)));
        sink.event(PreviewEvent::Interrupted);
        sink.event(PreviewEvent::Running);
        e.tick();
        assert_eq!(e.state.phase, PreviewPhase::Live);
        assert!(e.state.frame.is_none(), "中断前の画像は復活しない");
        assert_ne!(e.state.zoom, ZoomView::Value(2.0));
    }

    #[test]
    fn reset_pending_tracking_releases_static_session() {
        let drops = Arc::new(AtomicUsize::new(0));
        let session = FakeSession {
            drops: drops.clone(),
            zooms: Arc::new(AtomicUsize::new(0)),
        };
        *lock(&ACTIVE) = Some(Active {
            generation: 1,
            id: 0,
            shared: Arc::new(Shared::default()),
            last_zoom_seq: 0,
            session: Box::new(session),
        });
        reset_pending_tracking();
        assert!(lock(&ACTIVE).is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn only_one_start_per_request() {
        let mut e = Env::new(Fake::default());
        start(&mut e.state);
        e.tick();
        e.tick();
        assert_eq!(e.fake.starts.load(Ordering::SeqCst), 1);
    }
}
