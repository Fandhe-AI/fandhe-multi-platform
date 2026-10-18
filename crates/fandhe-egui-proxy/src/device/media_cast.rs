//! Phase 11 の拡充ループ（親 #99）型サンプル「外部機器への再生」（Issue #386）の effect ドライバ。
//! `fandhe_core::screens::cast_show`（端末依存ゼロ）の要求を `take_command` で観測し、
//! `fandhe_device::media_cast::MediaCast` を操作して、状態を `CompletionSender` 経由で注入する
//! （`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::media_cast`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_cast`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS: `apple_media_cast`、iOS: `ios_media_cast`、Windows: `windows_media_cast`。
//! macOS・Windows は実装済み、iOS は現状スタブで `UnsupportedMediaCast` を返す）。Web は別クレート
//! （`fandhe-web-dom`）。Android（Cast SDK が GMS 前提のため保留）と Linux は `UnsupportedMediaCast` 固定で、
//! ダミーの成功実装へは倒さない。
//!
//! # 並行性
//!
//! 実装は資源を 1 組だけ持つ（プロセスに 1 つ）。ドライバは `prepare` が返したデバイス世代と画面の要求世代を
//! 対応づけ、他の世代の snapshot は成功にしない。準備・再生・停止の期限と `Connecting` の期限は単調時計
//! （`Instant`）で測り、期限切れ後は `release` で資源を解放し、後から届く `Connected` も捨てる。
//! 状態は pull 型で、画面が `Idle` / `Failed` 以外の間 [`POLL_INTERVAL`] で再描画を予約する（契約表 4）。
//! 再生は利用者の操作に限り、画面を開いたときに自動で行うのは準備だけ。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
use fandhe_device::media_cast::UnsupportedMediaCast;
use fandhe_device::media_cast::{
    CastFailure, CastPlayback, CastSnapshot, MediaCast, PickerKind, RouteState, CONNECT_DEADLINE,
    POLL_INTERVAL, START_DEADLINE,
};
use fandhe_device::DeviceError;

use fandhe_core::screens::cast_show::{
    apply_update, expire, mark_command_taken, prepare, take_command, CastCommand, CastShowFailure,
    CastShowPlayback, CastShowRoute, CastShowState, CastShowUpdate, PickerView,
};

use crate::app::CompletionSender;

/// OS の生コードを含まない固定の失敗文言。
const FAILED_CAST: &str = "外部機器への再生";

/// プロセスに 1 つだけ持つ実装（`stop` が `play` と同じ実体に届くようにする）。
fn platform_cast() -> &'static Arc<dyn MediaCast> {
    static CAST: OnceLock<Arc<dyn MediaCast>> = OnceLock::new();
    CAST.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            super::apple_media_cast::implementation()
        }
        #[cfg(target_os = "ios")]
        {
            super::ios_media_cast::implementation()
        }
        #[cfg(target_os = "windows")]
        {
            super::windows_media_cast::implementation()
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
        {
            Arc::new(UnsupportedMediaCast)
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
    /// 画面を開いたときの自動準備を送信済みか。
    auto_prepared: bool,
    /// 現在の資源のデバイス世代（`prepare` の戻り値）。
    device_gen: Option<u64>,
    /// 遷移待ちの画面世代と起点（期限判定用、単調時計）。
    pending: Option<(u64, Instant)>,
    /// 経路が `Connecting` になった起点（単調時計）。
    connecting_since: Option<Instant>,
    /// 直近に snapshot を画面へ送った時刻。送信は即時再描画を起こすため間引く。
    last_report: Option<Instant>,
}

static SESSION: OnceLock<Session> = OnceLock::new();

/// 失敗・期限切れの確定時に、資源を再生状態を問わず全部解放し、ドライバの追跡も消す。
/// 先に `stop` を呼ぶのは、開始済みの再生を遅れて残さないため（`release` は冪等で再 `prepare` 可能）。
fn release_resources(inner: &mut SessionInner, cast: &Arc<dyn MediaCast>) {
    if let Some(g) = inner.device_gen.take() {
        let _ = cast.stop(g);
    }
    cast.release();
    inner.pending = None;
    inner.connecting_since = None;
}

fn to_failure(e: &DeviceError) -> CastShowFailure {
    match e {
        DeviceError::Unsupported(_) => CastShowFailure::Unsupported,
        DeviceError::PermissionDenied(_) => CastShowFailure::Blocked,
        // OS の生コードは画面へ出さない（security.md A09）。
        _ => CastShowFailure::Failed(FAILED_CAST.to_string()),
    }
}

fn snapshot_failure(f: CastFailure) -> CastShowFailure {
    match f {
        CastFailure::Unsupported => CastShowFailure::Unsupported,
        CastFailure::Blocked => CastShowFailure::Blocked,
        CastFailure::SourceNotSupported => CastShowFailure::SourceNotSupported,
        CastFailure::Timeout => CastShowFailure::Timeout,
        CastFailure::Os | CastFailure::Internal => CastShowFailure::Failed(FAILED_CAST.to_string()),
    }
}

fn route_view(r: &RouteState) -> CastShowRoute {
    match r {
        RouteState::Local => CastShowRoute::Local,
        RouteState::Choosing => CastShowRoute::Choosing,
        RouteState::Connecting => CastShowRoute::Connecting,
        RouteState::Connected { name } => CastShowRoute::Connected(name.clone()),
        RouteState::Disconnected => CastShowRoute::Disconnected,
        RouteState::NoDevices => CastShowRoute::NoDevices,
        RouteState::NotReported => CastShowRoute::NotReported,
    }
}

fn send_update(sender: &CompletionSender<CastShowState>, generation: u64, update: CastShowUpdate) {
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut CastShowState| {
        apply_update(s, generation, update);
    }));
}

/// 画面の状態に対して意味のある snapshot だけを通知へ写す。進行中（`Preparing` / `Starting` /
/// `Stopping`）は画面側が正で、デバイスの前状態の残りを成功として反映しない。
/// 返す `bool` は再生状態の通知（遷移の確定）を含むか。
fn updates_for(screen: &CastShowState, snap: &CastSnapshot) -> (Vec<CastShowUpdate>, bool) {
    use CastShowPlayback as S;
    let mut out = Vec::new();
    let picker = match snap.picker {
        PickerKind::Native => PickerView::Native,
        PickerKind::App => PickerView::App,
    };
    if screen.picker != Some(picker) {
        out.push(CastShowUpdate::Picker(picker));
    }
    let playback = match (&screen.playback, snap.playback) {
        (S::Preparing, CastPlayback::Ready) => Some(CastShowUpdate::Playback(S::Ready)),
        (S::Starting, CastPlayback::Playing) => Some(CastShowUpdate::Playback(S::Playing)),
        (S::Starting | S::Playing | S::Stopping, CastPlayback::Stopped) => {
            Some(CastShowUpdate::Playback(S::Stopped))
        }
        (S::Starting | S::Playing | S::Stopping, CastPlayback::Interrupted) => {
            Some(CastShowUpdate::Playback(S::Interrupted))
        }
        (
            S::Preparing | S::Ready | S::Starting | S::Playing | S::Stopping,
            CastPlayback::Failed(f),
        ) => Some(CastShowUpdate::Failed(snapshot_failure(f))),
        _ => None,
    };
    let confirmed = playback.is_some();
    let failed = matches!(playback, Some(CastShowUpdate::Failed(_)));
    out.extend(playback);
    if !failed {
        let route = route_view(&snap.route);
        if route != screen.route {
            out.push(CastShowUpdate::Route(route));
        }
    }
    (out, confirmed)
}

/// 1 フレームぶんの処理。要求の発送、snapshot の反映、期限判定を行い、
/// 次に再描画を予約した間隔（予約しなければ `None`）を返す。
fn drive(
    state: &CastShowState,
    sender: &CompletionSender<CastShowState>,
    cast: &Arc<dyn MediaCast>,
    session: &Session,
    deadline: Duration,
    connect_deadline: Duration,
) -> Option<Duration> {
    let mut inner = session.inner.lock().ok()?;

    // 画面を開いたら準備だけ自動で行う（再生は利用者の操作に限る）。
    if state.playback == CastShowPlayback::Idle && state.generation == 0 && !inner.auto_prepared {
        inner.auto_prepared = true;
        let _ = sender.send(Box::new(prepare));
        return None;
    }

    if let Some((command, generation)) = take_command(state) {
        if inner.handled != generation {
            inner.handled = generation;
            let _ = sender.send(Box::new(move |s: &mut CastShowState| {
                mark_command_taken(s, generation)
            }));
            inner.last_report = None;
            handle_command(&mut inner, sender, cast, command, generation);
        }
    }

    // 未発送の再生を画面側で取り消すと、要求なしで世代だけが進む。追跡を現在の世代へ揃える。
    if take_command(state).is_none() && state.generation != inner.handled {
        inner.handled = state.generation;
    }

    // 期限（単調時計）を過ぎたら、届いている結果も成功にせず失敗で確定し、遅れて始まる再生を止める。
    if state.generation == inner.handled {
        let transitioning = matches!(
            state.playback,
            CastShowPlayback::Preparing | CastShowPlayback::Starting | CastShowPlayback::Stopping
        );
        let expired_transition = transitioning
            && matches!(inner.pending, Some((g, t)) if g == state.generation && t.elapsed() >= deadline);
        if state.route == CastShowRoute::Connecting {
            inner.connecting_since.get_or_insert_with(Instant::now);
        } else {
            inner.connecting_since = None;
        }
        let expired_connect = inner
            .connecting_since
            .is_some_and(|t| t.elapsed() >= connect_deadline);
        if expired_transition || expired_connect {
            let g = state.generation;
            let _ = sender.send(Box::new(move |s: &mut CastShowState| expire(s, g)));
            release_resources(&mut inner, cast);
            return None;
        }
    }

    let live = !matches!(
        state.playback,
        CastShowPlayback::Idle | CastShowPlayback::Failed(_)
    );
    if live
        && state.generation == inner.handled
        && inner
            .last_report
            .is_none_or(|t| t.elapsed() >= POLL_INTERVAL)
    {
        inner.last_report = Some(Instant::now());
        report_snapshot(&mut inner, state, sender, cast);
    }

    if live {
        sender.request_repaint_after(POLL_INTERVAL);
        Some(POLL_INTERVAL)
    } else {
        None
    }
}

fn handle_command(
    inner: &mut SessionInner,
    sender: &CompletionSender<CastShowState>,
    cast: &Arc<dyn MediaCast>,
    command: CastCommand,
    generation: u64,
) {
    let fail = |inner: &mut SessionInner, e: &DeviceError| {
        release_resources(inner, cast);
        send_update(sender, generation, CastShowUpdate::Failed(to_failure(e)));
    };
    // 期限の起点は同期の呼び出しより前に取る（その所要時間も期限に含める）。
    let started = Instant::now();
    if command == CastCommand::Prepare {
        match cast.prepare(fandhe_device::media_playback::demo_wav()) {
            Ok(g) => {
                inner.device_gen = Some(g);
                inner.pending = Some((generation, started));
            }
            Err(e) => {
                inner.device_gen = None;
                fail(inner, &e);
            }
        }
        return;
    }
    let Some(g) = inner.device_gen else {
        return fail(inner, &DeviceError::Internal("media_cast"));
    };
    let r = match command {
        CastCommand::Play => cast.play(g),
        CastCommand::Stop => cast.stop(g),
        CastCommand::ShowPicker => match cast.show_picker(g) {
            // Native のピッカーは OS のボタンから選ぶ。要求が届いても失敗にはしない。
            Err(DeviceError::Unsupported(_)) => return,
            r => return r.unwrap_or_else(|e| fail(inner, &e)),
        },
        CastCommand::Prepare => return,
    };
    match r {
        Ok(()) => inner.pending = Some((generation, started)),
        Err(e) => fail(inner, &e),
    }
}

/// 現在の資源の snapshot を、世代が一致するときだけ画面へ送る。
fn report_snapshot(
    inner: &mut SessionInner,
    state: &CastShowState,
    sender: &CompletionSender<CastShowState>,
    cast: &Arc<dyn MediaCast>,
) {
    let Some(dg) = inner.device_gen else { return };
    let snap = cast.snapshot();
    if snap.generation != dg {
        return;
    }
    let (updates, confirmed) = updates_for(state, &snap);
    if confirmed {
        inner.pending = None;
    }
    let failed = updates
        .iter()
        .any(|u| matches!(u, CastShowUpdate::Failed(_)));
    if failed {
        release_resources(inner, cast);
    }
    for u in updates {
        send_update(sender, inner.handled, u);
    }
}

/// `registry.rs` の `effect` 区画から `cast_show` 画面用に配線される effect ドライバ。
pub fn cast_show_effect(state: &CastShowState, sender: &CompletionSender<CastShowState>) {
    drive(
        state,
        sender,
        platform_cast(),
        SESSION.get_or_init(Session::default),
        START_DEADLINE,
        CONNECT_DEADLINE,
    );
}

/// アプリ終了時に資源を解放する（`device::shutdown` から呼ばれる）。
pub(super) fn shutdown() {
    platform_cast().release();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::cast_show::{play, show_picker, stop};
    use std::sync::mpsc;

    struct FakeSt {
        gen: u64,
        playback: CastPlayback,
        route: RouteState,
        picker: PickerKind,
        prepare_err: Option<DeviceError>,
        play_err: Option<DeviceError>,
        auto_play: bool,
        releases: usize,
    }

    struct Fake(Mutex<FakeSt>);

    impl Fake {
        fn new(auto_play: bool) -> Arc<Self> {
            Arc::new(Self(Mutex::new(FakeSt {
                gen: 0,
                playback: CastPlayback::Idle,
                route: RouteState::Local,
                picker: PickerKind::App,
                prepare_err: None,
                play_err: None,
                auto_play,
                releases: 0,
            })))
        }
        fn set(&self, f: impl FnOnce(&mut FakeSt)) {
            f(&mut self.0.lock().unwrap());
        }
    }

    impl MediaCast for Fake {
        fn prepare(&self, wav: Arc<[u8]>) -> Result<u64, DeviceError> {
            let mut s = self.0.lock().unwrap();
            if wav.is_empty() {
                return Err(DeviceError::InvalidInput("media_cast wav"));
            }
            if let Some(e) = s.prepare_err.take() {
                return Err(e);
            }
            s.gen += 1;
            s.playback = CastPlayback::Ready;
            Ok(s.gen)
        }
        fn play(&self, g: u64) -> Result<(), DeviceError> {
            let mut s = self.0.lock().unwrap();
            if g != s.gen {
                return Err(DeviceError::InvalidInput("media_cast generation"));
            }
            if let Some(e) = s.play_err.take() {
                return Err(e);
            }
            if s.auto_play {
                s.playback = CastPlayback::Playing;
            }
            Ok(())
        }
        fn stop(&self, _g: u64) -> Result<(), DeviceError> {
            let mut s = self.0.lock().unwrap();
            if s.playback == CastPlayback::Playing {
                s.playback = CastPlayback::Stopped;
            }
            Ok(())
        }
        fn show_picker(&self, _g: u64) -> Result<(), DeviceError> {
            self.0.lock().unwrap().route = RouteState::Choosing;
            Ok(())
        }
        fn snapshot(&self) -> CastSnapshot {
            let s = self.0.lock().unwrap();
            CastSnapshot {
                generation: s.gen,
                playback: s.playback,
                route: s.route.clone(),
                picker: s.picker,
            }
        }
        fn release(&self) {
            self.0.lock().unwrap().releases += 1;
        }
    }

    type Rx = mpsc::Receiver<crate::app::Completion<CastShowState>>;

    struct H {
        fake: Arc<Fake>,
        cast: Arc<dyn MediaCast>,
        sender: CompletionSender<CastShowState>,
        rx: Rx,
        session: Session,
        state: CastShowState,
    }

    const D: Duration = Duration::from_secs(5);
    const CD: Duration = Duration::from_secs(15);

    impl H {
        fn new(fake: Arc<Fake>) -> Self {
            let cast: Arc<dyn MediaCast> = fake.clone();
            Self::with(fake, cast)
        }
        fn with(fake: Arc<Fake>, cast: Arc<dyn MediaCast>) -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                fake,
                cast,
                sender: CompletionSender::new(tx, egui::Context::default()),
                rx,
                session: Session::default(),
                state: CastShowState::default(),
            }
        }
        /// 間引きを無効にして 1 フレーム進める。
        fn drive(&mut self, d: Duration, cd: Duration) {
            self.session.inner.lock().unwrap().last_report = None;
            drive(&self.state, &self.sender, &self.cast, &self.session, d, cd);
            while let Ok(c) = self.rx.try_recv() {
                c(&mut self.state);
            }
        }
        fn step(&mut self) {
            self.drive(D, CD);
        }
        fn open_ready(&mut self) {
            self.step();
            assert_eq!(self.state.playback, CastShowPlayback::Preparing);
            self.step();
            self.step();
            assert_eq!(self.state.playback, CastShowPlayback::Ready);
        }
    }

    #[test]
    fn opens_ready_without_auto_play() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        assert_eq!(h.fake.0.lock().unwrap().playback, CastPlayback::Ready);
        assert_eq!(h.state.picker, Some(PickerView::App));
    }

    #[test]
    fn play_then_stop_reaches_stopped() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        play(&mut h.state);
        h.step();
        h.step();
        assert_eq!(h.state.playback, CastShowPlayback::Playing);
        stop(&mut h.state);
        h.step();
        h.step();
        assert_eq!(h.state.playback, CastShowPlayback::Stopped);
    }

    #[test]
    fn prepare_invalid_input_fails_and_releases() {
        let fake = Fake::new(true);
        fake.set(|s| s.prepare_err = Some(DeviceError::InvalidInput("media_cast wav")));
        let mut h = H::new(fake);
        h.step();
        h.step();
        h.step();
        assert!(matches!(
            h.state.playback,
            CastShowPlayback::Failed(CastShowFailure::Failed(_))
        ));
        assert_eq!(h.fake.0.lock().unwrap().releases, 1);
    }

    #[test]
    fn stale_device_generation_is_not_success() {
        let mut h = H::new(Fake::new(false));
        h.open_ready();
        play(&mut h.state);
        h.step();
        h.fake.set(|s| {
            s.gen += 1;
            s.playback = CastPlayback::Playing;
        });
        h.step();
        assert_eq!(h.state.playback, CastShowPlayback::Starting);
    }

    #[test]
    fn starting_deadline_times_out_releases_and_drops_late_playing() {
        let mut h = H::new(Fake::new(false));
        h.open_ready();
        play(&mut h.state);
        h.step();
        assert_eq!(h.state.playback, CastShowPlayback::Starting);
        let releases = h.fake.0.lock().unwrap().releases;
        h.drive(Duration::ZERO, CD);
        assert_eq!(
            h.state.playback,
            CastShowPlayback::Failed(CastShowFailure::Timeout)
        );
        assert_eq!(h.fake.0.lock().unwrap().releases, releases + 1);
        h.fake.set(|s| s.playback = CastPlayback::Playing);
        h.step();
        assert_eq!(
            h.state.playback,
            CastShowPlayback::Failed(CastShowFailure::Timeout)
        );
    }

    #[test]
    fn connecting_deadline_drops_late_connected() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        play(&mut h.state);
        h.step();
        h.step();
        h.fake.set(|s| s.route = RouteState::Connecting);
        h.step();
        assert_eq!(h.state.route, CastShowRoute::Connecting);
        h.step();
        h.drive(D, Duration::ZERO);
        assert_eq!(
            h.state.playback,
            CastShowPlayback::Failed(CastShowFailure::Timeout)
        );
        h.fake.set(|s| {
            s.route = RouteState::Connected { name: None };
        });
        h.step();
        assert_eq!(h.state.route, CastShowRoute::Local);
    }

    #[test]
    fn picker_cancel_is_not_a_failure() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        show_picker(&mut h.state);
        h.step();
        h.step();
        assert_eq!(h.state.route, CastShowRoute::Choosing);
        // ピッカーを閉じた（キャンセル）
        h.fake.set(|s| s.route = RouteState::Local);
        h.step();
        assert_eq!(h.state.route, CastShowRoute::Local);
        assert_eq!(h.state.playback, CastShowPlayback::Ready);
    }

    #[test]
    fn permission_denied_is_blocked() {
        let fake = Fake::new(true);
        let mut h = H::new(fake.clone());
        h.open_ready();
        fake.set(|s| s.play_err = Some(DeviceError::PermissionDenied("x")));
        play(&mut h.state);
        h.step();
        h.step();
        assert_eq!(
            h.state.playback,
            CastShowPlayback::Failed(CastShowFailure::Blocked)
        );
    }

    #[test]
    fn unsupported_implementation_never_plays_or_connects() {
        let mut h = H::with(
            Fake::new(true),
            Arc::new(fandhe_device::media_cast::UnsupportedMediaCast),
        );
        h.step();
        h.step();
        h.step();
        assert_eq!(
            h.state.playback,
            CastShowPlayback::Failed(CastShowFailure::Unsupported)
        );
        play(&mut h.state);
        show_picker(&mut h.state);
        assert_eq!(h.state.command, None);
        assert_eq!(h.state.route, CastShowRoute::Local);
    }

    #[test]
    fn native_picker_kind_is_reported_and_blocks_request() {
        let fake = Fake::new(true);
        fake.set(|s| s.picker = PickerKind::Native);
        let mut h = H::new(fake);
        h.open_ready();
        assert_eq!(h.state.picker, Some(PickerView::Native));
        show_picker(&mut h.state);
        assert_eq!(h.state.command, None);
    }

    #[test]
    fn command_is_not_handled_twice() {
        let mut h = H::new(Fake::new(true));
        h.step();
        let c = take_command(&h.state);
        assert!(c.is_some());
        h.step();
        h.state.command = c.map(|(c, _)| c);
        h.step();
        assert_eq!(h.fake.0.lock().unwrap().gen, 1);
    }

    #[test]
    fn cancelled_unsent_play_keeps_tracking_current_generation() {
        let mut h = H::new(Fake::new(true));
        h.open_ready();
        play(&mut h.state);
        stop(&mut h.state);
        h.step();
        assert_eq!(h.session.inner.lock().unwrap().handled, h.state.generation);
        show_picker(&mut h.state);
        h.step();
        assert_eq!(h.state.command, None);
    }
}
