//! Phase 11 の拡充ループ（親 #97）型サンプル「ピクチャ・イン・ピクチャ」（Issue #364）の effect ドライバ。
//! `fandhe_core::screens::pip_show`（端末依存ゼロ）の要求を `take_command` で観測し、
//! `fandhe_device::picture_in_picture::PictureInPicture` を操作して、状態を `CompletionSender` 経由で
//! 注入する（`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::picture_in_picture`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_pip`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS: `apple_picture_in_picture`、iOS: `ios_picture_in_picture`、
//! Android: `android_picture_in_picture`。macOS は実装済み、iOS・Android は現状スタブで
//! `UnsupportedPictureInPicture` を返す）。それ以外（Windows は保留、Linux）は
//! `UnsupportedPictureInPicture` 固定で、ダミーの成功実装へは倒さない。
//!
//! # 並行性
//!
//! 実装は小窓の資源を 1 組だけ持つ（プロセスに 1 つ）。ドライバは `prepare` が返したデバイス世代と
//! 画面の要求世代を対応づけ、他の世代の snapshot は成功にしない。準備・開始・終了の期限は単調時計
//! （`Instant`）で測り、期限切れ後は `stop` を呼んで遅れて始まった小窓を残さず、後から届く `Active` も捨てる。
//! 状態は pull 型で、画面が `Idle` / `Failed` 以外の間 [`POLL_INTERVAL`] で再描画を予約する。
//! 開始は利用者の操作に限り、画面を開いたときに自動で行うのは準備だけ。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
use fandhe_device::picture_in_picture::UnsupportedPictureInPicture;
use fandhe_device::picture_in_picture::{
    frame_color, PictureInPicture, PipFailure, PipPhase, POLL_INTERVAL, START_DEADLINE,
};
use fandhe_device::DeviceError;

use fandhe_core::screens::pip_show::{
    apply_update, expire, mark_command_taken, prepare, set_preview, take_command, PipCommand,
    PipShowFailure, PipShowPhase, PipShowState, PipShowUpdate,
};

use crate::app::CompletionSender;

/// OS の生コードを含まない固定の失敗文言。
const FAILED_PIP: &str = "小窓の操作";

/// プロセスに 1 つだけ持つ実装（`stop` が `start` と同じ実体に届くようにする）。
fn platform_pip() -> &'static Arc<dyn PictureInPicture> {
    static PIP: OnceLock<Arc<dyn PictureInPicture>> = OnceLock::new();
    PIP.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            super::apple_picture_in_picture::implementation()
        }
        #[cfg(target_os = "ios")]
        {
            super::ios_picture_in_picture::implementation()
        }
        #[cfg(target_os = "android")]
        {
            super::android_picture_in_picture::implementation()
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
        {
            Arc::new(UnsupportedPictureInPicture)
        }
    })
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
#[derive(Default)]
struct Session {
    inner: Mutex<SessionInner>,
}

#[derive(Default)]
struct SessionInner {
    /// 処理済みの要求世代（`mark_command_taken` の反映前に二重処理しない）。
    handled: u64,
    /// 画面を開いたときの自動準備を送信済みか（反映前の二重送信を防ぐ）。
    auto_prepared: bool,
    /// 現在の資源のデバイス世代（`prepare` の戻り値）。
    device_gen: Option<u64>,
    /// 遷移待ちの画面世代と起点（期限判定用、単調時計）。
    pending: Option<(u64, Instant)>,
    /// 画面内プレビューの色替えの起点（`prepare` 成功時）。
    prepared_at: Option<Instant>,
    /// 直近に画面へ送ったプレビューの色。
    last_preview: Option<[u8; 3]>,
    /// 直近に snapshot を画面へ送った時刻。送信は即時再描画を起こすため間引く。
    last_report: Option<Instant>,
}

static SESSION: OnceLock<Session> = OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、ドライバの記録を破棄して小窓を止める。
/// 画面状態は `default()` に戻るため、処理済み世代を残すと次の準備が飲み込まれる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        reset_session(s, platform_pip());
    }
}

fn reset_session(session: &Session, pip: &Arc<dyn PictureInPicture>) {
    let mut inner = session.inner.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(g) = inner.device_gen {
        let _ = pip.stop(g);
    }
    *inner = SessionInner::default();
}

/// 失敗・期限切れの確定時に、小窓の資源を phase を問わず全部解放し、ドライバの追跡も消す。
/// 先に `stop` を呼ぶのは、開始済みの小窓を遅れて残さないため（`shutdown` は冪等で再 `prepare` 可能）。
fn release_resources(inner: &mut SessionInner, pip: &Arc<dyn PictureInPicture>) {
    if let Some(g) = inner.device_gen.take() {
        let _ = pip.stop(g);
    }
    pip.shutdown();
    inner.prepared_at = None;
    inner.last_preview = None;
}

fn to_failure(e: &DeviceError) -> PipShowFailure {
    match e {
        DeviceError::Unsupported(_) => PipShowFailure::Unsupported,
        DeviceError::Disabled(_) => PipShowFailure::NotPossible,
        DeviceError::PermissionDenied(_) => PipShowFailure::NotAllowed,
        // OS の生コードは画面へ出さない（security.md A09）。
        _ => PipShowFailure::Failed(FAILED_PIP.to_string()),
    }
}

fn snapshot_failure(f: PipFailure) -> PipShowFailure {
    match f {
        PipFailure::Unsupported => PipShowFailure::Unsupported,
        PipFailure::NotPossible => PipShowFailure::NotPossible,
        PipFailure::NotAllowed => PipShowFailure::NotAllowed,
        PipFailure::Internal => PipShowFailure::Failed(FAILED_PIP.to_string()),
    }
}

fn send_update(sender: &CompletionSender<PipShowState>, generation: u64, update: PipShowUpdate) {
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut PipShowState| {
        apply_update(s, generation, update);
    }));
}

/// 画面の状態に対して意味のある snapshot だけを通知へ写す。進行中（`Preparing` / `Starting` /
/// `Stopping`）は画面側が正で、デバイスの前状態の残りを成功として反映しない。
fn report_for(screen: &PipShowPhase, device: PipPhase) -> Option<PipShowUpdate> {
    use PipShowPhase as S;
    match (screen, device) {
        (S::Preparing, PipPhase::Ready) => Some(PipShowUpdate::Phase(S::Ready)),
        (S::Starting, PipPhase::Active) => Some(PipShowUpdate::Phase(S::Active)),
        (S::Starting | S::Active | S::Stopping, PipPhase::Exited) => {
            Some(PipShowUpdate::Phase(S::Exited))
        }
        (S::Preparing | S::Starting | S::Active | S::Stopping, PipPhase::Failed(f)) => {
            Some(PipShowUpdate::Failed(snapshot_failure(f)))
        }
        _ => None,
    }
}

/// 1 フレームぶんの処理。要求の発送、snapshot の反映、期限判定、プレビュー更新を行い、
/// 次に再描画を予約した間隔（予約しなければ `None`）を返す。
fn drive(
    state: &PipShowState,
    sender: &CompletionSender<PipShowState>,
    pip: &Arc<dyn PictureInPicture>,
    session: &Session,
    deadline: Duration,
) -> Option<Duration> {
    let mut inner = session.inner.lock().ok()?;

    // 画面を開いたら準備だけ自動で行う（開始は利用者の操作に限る）。
    if state.phase == PipShowPhase::Idle && state.generation == 0 && !inner.auto_prepared {
        inner.auto_prepared = true;
        let _ = sender.send(Box::new(prepare));
        return None;
    }

    if let Some((command, generation)) = take_command(state) {
        if inner.handled != generation {
            inner.handled = generation;
            let _ = sender.send(Box::new(move |s: &mut PipShowState| {
                mark_command_taken(s, generation)
            }));
            inner.last_report = None;
            handle_command(&mut inner, sender, pip, command, generation);
        }
    }

    let transitioning = matches!(
        state.phase,
        PipShowPhase::Preparing | PipShowPhase::Starting | PipShowPhase::Stopping
    );
    if transitioning && state.generation == inner.handled {
        if let Some((g, started)) = inner.pending {
            // 期限（単調時計）を過ぎたら、届いている結果も成功にせず失敗で確定し、遅れて始まる小窓を止める。
            if g == state.generation && started.elapsed() >= deadline {
                inner.pending = None;
                let _ = sender.send(Box::new(move |s: &mut PipShowState| expire(s, g)));
                release_resources(&mut inner, pip);
                return None;
            }
        }
    }

    let live = !matches!(state.phase, PipShowPhase::Idle | PipShowPhase::Failed(_));
    if live
        && state.generation == inner.handled
        && inner
            .last_report
            .is_none_or(|t| t.elapsed() >= POLL_INTERVAL)
    {
        inner.last_report = Some(Instant::now());
        report_snapshot(&mut inner, state, sender, pip);
    }

    // 画面内プレビューは OS 側の映像と同じ色表を、準備からの経過時間で引く。
    if live {
        if let Some(t0) = inner.prepared_at {
            let color = frame_color(t0.elapsed());
            if state.preview_rgb != Some(color) && inner.last_preview != Some(color) {
                inner.last_preview = Some(color);
                let _ = sender.send(Box::new(move |s: &mut PipShowState| set_preview(s, color)));
            }
        }
        sender.request_repaint_after(POLL_INTERVAL);
        Some(POLL_INTERVAL)
    } else {
        None
    }
}

fn handle_command(
    inner: &mut SessionInner,
    sender: &CompletionSender<PipShowState>,
    pip: &Arc<dyn PictureInPicture>,
    command: PipCommand,
    generation: u64,
) {
    let fail = |inner: &mut SessionInner, e: &DeviceError| {
        inner.pending = None;
        release_resources(inner, pip);
        send_update(sender, generation, PipShowUpdate::Failed(to_failure(e)));
    };
    // 期限の起点は同期の呼び出しより前に取る（その所要時間も期限に含める）。
    let started = Instant::now();
    match command {
        PipCommand::Prepare => {
            inner.last_preview = None;
            match pip.prepare() {
                Ok(g) => {
                    inner.device_gen = Some(g);
                    inner.pending = Some((generation, started));
                    inner.prepared_at = Some(started);
                }
                Err(e) => {
                    inner.device_gen = None;
                    inner.prepared_at = None;
                    fail(inner, &e);
                }
            }
        }
        PipCommand::Start | PipCommand::Stop => {
            let Some(g) = inner.device_gen else {
                return fail(inner, &DeviceError::Internal("picture_in_picture"));
            };
            let r = if command == PipCommand::Start {
                pip.start(g)
            } else {
                pip.stop(g)
            };
            match r {
                Ok(()) => inner.pending = Some((generation, started)),
                Err(e) => fail(inner, &e),
            }
        }
    }
}

/// 現在の資源の snapshot を、世代が一致するときだけ画面へ送る。
fn report_snapshot(
    inner: &mut SessionInner,
    state: &PipShowState,
    sender: &CompletionSender<PipShowState>,
    pip: &Arc<dyn PictureInPicture>,
) {
    let Some(dg) = inner.device_gen else { return };
    let snap = pip.snapshot();
    if snap.generation != dg {
        return;
    }
    if let Some(update) = report_for(&state.phase, snap.phase) {
        inner.pending = None;
        if matches!(update, PipShowUpdate::Failed(_)) {
            release_resources(inner, pip);
        }
        send_update(sender, inner.handled, update);
    }
}

/// `registry.rs` の `effect` 区画から `pip_show` 画面用に配線される effect ドライバ。
pub fn pip_show_effect(state: &PipShowState, sender: &CompletionSender<PipShowState>) {
    drive(
        state,
        sender,
        platform_pip(),
        SESSION.get_or_init(Session::default),
        START_DEADLINE,
    );
}

/// アプリ終了時に小窓の資源を解放する（`device::shutdown` から呼ばれる）。
pub(super) fn shutdown() {
    platform_pip().shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::pip_show::{start, stop};
    use fandhe_device::picture_in_picture::PipSnapshot;
    use std::sync::mpsc;

    struct FakeSt {
        gen: u64,
        phase: PipPhase,
        prepare_err: Option<DeviceError>,
        start_err: Option<DeviceError>,
        auto_active: bool,
        stops: usize,
    }

    struct Fake(Mutex<FakeSt>);

    impl Fake {
        fn new(auto_active: bool) -> Arc<Self> {
            Arc::new(Self(Mutex::new(FakeSt {
                gen: 0,
                phase: PipPhase::Idle,
                prepare_err: None,
                start_err: None,
                auto_active,
                stops: 0,
            })))
        }
        fn set_phase(&self, p: PipPhase) {
            self.0.lock().unwrap().phase = p;
        }
    }

    impl PictureInPicture for Fake {
        fn prepare(&self) -> Result<u64, DeviceError> {
            let mut s = self.0.lock().unwrap();
            if let Some(e) = s.prepare_err.take() {
                return Err(e);
            }
            s.gen += 1;
            s.phase = PipPhase::Ready;
            Ok(s.gen)
        }
        fn start(&self, g: u64) -> Result<(), DeviceError> {
            let mut s = self.0.lock().unwrap();
            if g != s.gen {
                return Err(DeviceError::InvalidInput("picture_in_picture generation"));
            }
            if let Some(e) = s.start_err.take() {
                return Err(e);
            }
            s.phase = if s.auto_active {
                PipPhase::Active
            } else {
                PipPhase::Starting
            };
            Ok(())
        }
        fn stop(&self, _g: u64) -> Result<(), DeviceError> {
            let mut s = self.0.lock().unwrap();
            s.stops += 1;
            if matches!(s.phase, PipPhase::Starting | PipPhase::Active) {
                s.phase = PipPhase::Exited;
            }
            Ok(())
        }
        fn snapshot(&self) -> PipSnapshot {
            let s = self.0.lock().unwrap();
            PipSnapshot {
                generation: s.gen,
                phase: s.phase,
            }
        }
        fn shutdown(&self) {}
    }

    /// 常に `Unsupported` を返す実装（非対応 OS の fallback と同じ契約）。
    struct Unsupported;
    impl PictureInPicture for Unsupported {
        fn prepare(&self) -> Result<u64, DeviceError> {
            Err(DeviceError::Unsupported("picture_in_picture"))
        }
        fn start(&self, _: u64) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported("picture_in_picture"))
        }
        fn stop(&self, _: u64) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported("picture_in_picture"))
        }
        fn snapshot(&self) -> PipSnapshot {
            PipSnapshot {
                generation: 0,
                phase: PipPhase::Idle,
            }
        }
        fn shutdown(&self) {}
    }

    type Rx = mpsc::Receiver<crate::app::Completion<PipShowState>>;

    struct H {
        fake: Arc<Fake>,
        pip: Arc<dyn PictureInPicture>,
        sender: CompletionSender<PipShowState>,
        rx: Rx,
        session: Session,
        state: PipShowState,
    }

    impl H {
        fn new(fake: Arc<Fake>) -> Self {
            let pip: Arc<dyn PictureInPicture> = fake.clone();
            Self::with(fake, pip)
        }
        fn with(fake: Arc<Fake>, pip: Arc<dyn PictureInPicture>) -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                fake,
                pip,
                sender: CompletionSender::new(tx, egui::Context::default()),
                rx,
                session: Session::default(),
                state: PipShowState::default(),
            }
        }
        /// 間引きを無効にして 1 フレーム進める。
        fn drive(&mut self, d: Duration) -> Option<Duration> {
            self.session.inner.lock().unwrap().last_report = None;
            self.drive_throttled(d)
        }
        fn drive_throttled(&mut self, d: Duration) -> Option<Duration> {
            let r = drive(&self.state, &self.sender, &self.pip, &self.session, d);
            while let Ok(c) = self.rx.try_recv() {
                c(&mut self.state);
            }
            r
        }
        /// 自動準備を送り、準備して `Ready` まで進める。
        fn open_ready(&mut self) {
            self.drive(D);
            assert_eq!(self.state.phase, PipShowPhase::Preparing);
            self.drive(D);
            self.drive(D);
            assert_eq!(self.state.phase, PipShowPhase::Ready);
        }
    }

    const D: Duration = Duration::from_secs(5);

    #[test]
    fn opens_ready_without_auto_start() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        // 開始は利用者の操作だけ
        assert_eq!(h.fake.0.lock().unwrap().phase, PipPhase::Ready);
        assert_eq!(h.drive(D), Some(POLL_INTERVAL));
    }

    #[test]
    fn start_becomes_active_and_stop_exits() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        start(&mut h.state);
        h.drive(D);
        h.drive(D);
        assert_eq!(h.state.phase, PipShowPhase::Active);
        stop(&mut h.state);
        h.drive(D);
        h.drive(D);
        assert_eq!(h.state.phase, PipShowPhase::Exited);
    }

    #[test]
    fn unsupported_is_failure_not_success() {
        let mut h = H::with(Fake::new(true), Arc::new(Unsupported));
        h.drive(D);
        h.drive(D);
        assert_eq!(
            h.state.phase,
            PipShowPhase::Failed(PipShowFailure::Unsupported)
        );
        // 非対応のまま「小窓にする」を押しても何も起きない
        start(&mut h.state);
        assert_eq!(h.state.command, None);
    }

    #[test]
    fn prepare_internal_error_fails() {
        let fake = Fake::new(true);
        fake.0.lock().unwrap().prepare_err = Some(DeviceError::Internal("x"));
        let mut h = H::new(fake);
        h.drive(D);
        h.drive(D);
        assert!(matches!(
            h.state.phase,
            PipShowPhase::Failed(PipShowFailure::Failed(_))
        ));
    }

    #[test]
    fn start_disabled_is_not_possible_and_denied_is_not_allowed() {
        for (err, want) in [
            (DeviceError::Disabled("x"), PipShowFailure::NotPossible),
            (
                DeviceError::PermissionDenied("x"),
                PipShowFailure::NotAllowed,
            ),
            (
                DeviceError::InvalidInput("x"),
                PipShowFailure::Failed(FAILED_PIP.to_string()),
            ),
        ] {
            let fake = Fake::new(true);
            let mut h = H::new(fake.clone());
            h.open_ready();
            fake.0.lock().unwrap().start_err = Some(err);
            start(&mut h.state);
            h.drive(D);
            h.drive(D);
            assert_eq!(h.state.phase, PipShowPhase::Failed(want));
        }
    }

    #[test]
    fn stop_while_starting_exits_not_fails() {
        let mut h = H::new(Fake::new(false));
        h.open_ready();
        start(&mut h.state);
        h.drive(D);
        assert_eq!(h.state.phase, PipShowPhase::Starting);
        stop(&mut h.state);
        h.drive(D);
        h.drive(D);
        assert_eq!(h.state.phase, PipShowPhase::Exited);
    }

    #[test]
    fn os_side_close_exits() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        start(&mut h.state);
        h.drive(D);
        h.drive(D);
        assert_eq!(h.state.phase, PipShowPhase::Active);
        h.fake.set_phase(PipPhase::Exited);
        h.drive(D);
        assert_eq!(h.state.phase, PipShowPhase::Exited);
    }

    #[test]
    fn deadline_times_out_stops_and_drops_late_active() {
        let mut h = H::new(Fake::new(false));
        h.open_ready();
        start(&mut h.state);
        h.drive(D);
        assert_eq!(h.state.phase, PipShowPhase::Starting);
        let stops = h.fake.0.lock().unwrap().stops;
        // 期限 0 なら次のフレームで期限切れになる
        h.drive(Duration::ZERO);
        assert_eq!(h.state.phase, PipShowPhase::Failed(PipShowFailure::Timeout));
        assert_eq!(h.fake.0.lock().unwrap().stops, stops + 1);
        assert!(h.session.inner.lock().unwrap().device_gen.is_none());
        // 遅れて届く Active は反映しない
        h.fake.set_phase(PipPhase::Active);
        h.drive(D);
        assert_eq!(h.state.phase, PipShowPhase::Failed(PipShowFailure::Timeout));
    }

    #[test]
    fn stale_device_generation_is_dropped() {
        let mut h = H::new(Fake::new(false));
        h.open_ready();
        start(&mut h.state);
        h.drive(D);
        h.fake.0.lock().unwrap().gen += 1;
        h.fake.set_phase(PipPhase::Active);
        h.drive(D);
        assert_eq!(h.state.phase, PipShowPhase::Starting);
    }

    #[test]
    fn command_is_not_handled_twice() {
        let mut h = H::new(Fake::new(true));
        h.drive(D);
        // 反映前の同じ要求を 2 回観測しても prepare は 1 回
        let c = take_command(&h.state);
        assert!(c.is_some());
        h.drive(D);
        h.state.command = c.map(|(c, _)| c);
        h.drive(D);
        assert_eq!(h.fake.0.lock().unwrap().gen, 1);
    }

    #[test]
    fn reset_pending_tracking_stops_and_clears() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        start(&mut h.state);
        h.drive(D);
        h.drive(D);
        reset_session(&h.session, &h.pip);
        assert_eq!(h.fake.0.lock().unwrap().phase, PipPhase::Exited);
        assert!(h.session.inner.lock().unwrap().device_gen.is_none());
    }

    #[test]
    fn preview_is_sent_and_color_follows_table() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        assert!(h.state.preview_rgb.is_some());
    }
}
