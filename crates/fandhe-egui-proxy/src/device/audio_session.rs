//! Phase 11 の拡充ループ（親 #172）型サンプル「音声の割り込み・セッション種別」（Issue #840）の
//! effect ドライバ。`fandhe_core::screens::audio_session_show`（端末依存ゼロ）の要求を
//! `take_command` で観測し、`fandhe_device::audio_session::AudioSession` でセッションを開始 / 停止し、
//! 届いた出来事と読み戻した状態を `CompletionSender` 経由で注入する
//! （`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::audio_session`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_session`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（iOS: `ios_audio_session`（#841）、Android: `android_audio_session`（#842。
//! 再生成時は `reset_pending_tracking` を `android_main` が呼ぶ））。それ以外は
//! `UnsupportedAudioSession` 固定で、ダミーの成功実装へは倒さない。
//!
//! # 並行性
//!
//! 開始・停止・解放・読み戻しはすべて UI スレッドの 1 本の effect（[`drive`]）で行い、OS コールバックは
//! 世代つきの完了クロージャを積むだけにする。握っているセッション（guard）は 1 つで、その世代と共に
//! 1 つのロックの内側にある。停止の後に遅れて届いた通知は画面側の世代照合で捨てる。実行中の失敗で
//! 画面が `Failed` になったら、次のフレームでこのドライバが guard を解放する。タイマー・経過時間は使わない。

use std::sync::{Arc, Mutex, OnceLock};

use fandhe_core::screens::audio_session_show::{
    apply_update, mark_command_taken, take_command, AudioModeView, AudioSessionCommand,
    AudioSessionFailure, AudioSessionPhase, AudioSessionShowState, AudioSessionUpdate, CurrentView,
    EventKindView, EventRow, KindChoice, ReadKindView, SupportView,
};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_device::audio_session::UnsupportedAudioSession;
use fandhe_device::audio_session::{
    AudioMode, AudioSession, AudioSessionGuard, EventKind, OnSignal, ReadKind, SessionKind, Signal,
    StartFailure, Support,
};

use crate::app::CompletionSender;

/// プロセスに 1 つだけ持つ実装。
fn platform_session() -> &'static Arc<dyn AudioSession> {
    static SESSION_IMPL: OnceLock<Arc<dyn AudioSession>> = OnceLock::new();
    SESSION_IMPL.get_or_init(|| {
        #[cfg(target_os = "ios")]
        {
            super::ios_audio_session::implementation()
        }
        #[cfg(target_os = "android")]
        {
            super::android_audio_session::implementation()
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Arc::new(UnsupportedAudioSession)
        }
    })
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
#[derive(Default)]
struct Driver {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// 処理済みの要求世代（`mark_command_taken` の反映前に二重処理しない）。
    handled: u64,
    /// 対応範囲を画面へ注入済みか。
    support_sent: bool,
    /// 握っているセッションとその世代。
    guard: Option<(u64, Box<dyn AudioSessionGuard>)>,
    /// 読み戻し済みの出来事番号と読み直し回数。
    read_seq: u64,
    read_epoch: u64,
}

static DRIVER: OnceLock<Driver> = OnceLock::new();

impl Driver {
    /// 握っているセッションを解放して記録を捨てる。poison は無視して進める。
    fn reset(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, g)) = inner.guard.take() {
            let _ = g.stop();
        }
        *inner = Inner::default();
    }
}

/// Activity 再生成時（`android_main` の再実行）に、握っているセッションを解放し、ドライバの記録を
/// 破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の「開始」が処理済みとして
/// 飲み込まれる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(d) = DRIVER.get() {
        d.reset();
    }
}

/// アプリ終了時にセッションを解放する（`device::shutdown` から呼ばれる）。
pub(super) fn shutdown() {
    reset_pending_tracking();
}

fn to_support_view(s: &Support) -> SupportView {
    SupportView {
        kinds: s.kinds,
        kind_readable: s.kind_readable,
        other_playing_readable: s.other_playing_readable,
        audio_mode_readable: s.audio_mode_readable,
        music_active_readable: s.music_active_readable,
    }
}

fn to_session_kind(k: KindChoice) -> SessionKind {
    match k {
        KindChoice::StopOthers => SessionKind::StopOthers,
        KindChoice::DuckOthers => SessionKind::DuckOthers,
        KindChoice::MixWithOthers => SessionKind::MixWithOthers,
    }
}

fn to_event_kind(k: EventKind) -> EventKindView {
    match k {
        EventKind::InterruptionBegan => EventKindView::InterruptionBegan,
        EventKind::InterruptionEnded => EventKindView::InterruptionEnded,
        EventKind::RouteChanged => EventKindView::RouteChanged,
        EventKind::FocusGained => EventKindView::FocusGained,
        EventKind::FocusLost => EventKindView::FocusLost,
        EventKind::FocusLostTransient => EventKindView::FocusLostTransient,
        EventKind::FocusLostTransientCanDuck => EventKindView::FocusLostTransientCanDuck,
        EventKind::BecomingNoisy => EventKindView::BecomingNoisy,
    }
}

fn to_read_kind(k: ReadKind) -> ReadKindView {
    match k {
        ReadKind::StopOthers => ReadKindView::StopOthers,
        ReadKind::DuckOthers => ReadKindView::DuckOthers,
        ReadKind::MixWithOthers => ReadKindView::MixWithOthers,
        ReadKind::Other => ReadKindView::Other,
    }
}

fn to_audio_mode(m: AudioMode) -> AudioModeView {
    match m {
        AudioMode::Normal => AudioModeView::Normal,
        AudioMode::Ringtone => AudioModeView::Ringtone,
        AudioMode::InCall => AudioModeView::InCall,
        AudioMode::InCommunication => AudioModeView::InCommunication,
        AudioMode::CallScreening => AudioModeView::CallScreening,
        AudioMode::Redirect => AudioModeView::Redirect,
        AudioMode::Unknown => AudioModeView::Unknown,
    }
}

fn to_failure(f: StartFailure) -> AudioSessionFailure {
    match f {
        StartFailure::Unsupported => AudioSessionFailure::Unsupported,
        StartFailure::KindUnsupported => AudioSessionFailure::KindUnsupported,
        StartFailure::Rejected => AudioSessionFailure::Rejected,
        StartFailure::Failed { restored } => AudioSessionFailure::StartFailed { restored },
    }
}

fn send_update(
    sender: &CompletionSender<AudioSessionShowState>,
    generation: u64,
    update: AudioSessionUpdate,
) {
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut AudioSessionShowState| {
        apply_update(s, generation, update);
    }));
}

/// OS から今の状態を読み戻して注入する。アプリ内の記憶から作らない。
fn read_current(
    sender: &CompletionSender<AudioSessionShowState>,
    session: &Arc<dyn AudioSession>,
    generation: u64,
) {
    let result = session
        .read_current()
        .map(|c| CurrentView {
            kind: c.kind.map(to_read_kind),
            other_audio_playing: c.other_audio_playing,
            audio_mode: c.audio_mode.map(to_audio_mode),
            music_active: c.music_active,
        })
        .map_err(|_| ());
    send_update(sender, generation, AudioSessionUpdate::Current(result));
}

/// 停止して、戻せなかったら画面へ知らせる。
fn release(
    sender: &CompletionSender<AudioSessionShowState>,
    generation: u64,
    guard: Box<dyn AudioSessionGuard>,
) {
    if guard.stop().is_err() {
        send_update(sender, generation, AudioSessionUpdate::RestoreFailed);
    }
}

/// OS コールバックから呼ばれる通知先。世代つきの完了クロージャを積むだけで、すぐ戻る。
fn make_on_signal(sender: CompletionSender<AudioSessionShowState>, generation: u64) -> OnSignal {
    Arc::new(move |signal: Signal| {
        let update = match signal {
            Signal::Event(e) => AudioSessionUpdate::Event(EventRow {
                kind: to_event_kind(e.kind),
                should_resume: e.should_resume,
            }),
            Signal::Failed => AudioSessionUpdate::Failed(AudioSessionFailure::Runtime),
        };
        send_update(&sender, generation, update);
    })
}

/// 1 フレームぶんの処理。対応範囲の注入、失敗後の解放、要求の発送、読み戻しを行う。
fn drive(
    state: &AudioSessionShowState,
    sender: &CompletionSender<AudioSessionShowState>,
    session: &Arc<dyn AudioSession>,
    driver: &Driver,
) {
    let Ok(mut inner) = driver.inner.lock() else {
        return;
    };
    let support = session.support();
    let readable = to_support_view(&support).any_readable();
    if !inner.support_sent {
        inner.support_sent = true;
        send_update(
            sender,
            state.generation,
            AudioSessionUpdate::Support(to_support_view(&support)),
        );
        if readable {
            read_current(sender, session, state.generation);
        }
    }

    // 実行中の失敗で画面が `Failed` になった guard はここで解放する。利用者の停止（世代が進む）は
    // `Stop` の要求側で、古い世代の guard が残る開始し直しは `Start` 側で解放する。
    if matches!(state.phase, AudioSessionPhase::Failed(_)) {
        if let Some((g, guard)) = inner.guard.take() {
            release(sender, g, guard);
        }
    }

    if let Some((command, generation)) = take_command(state) {
        if inner.handled != generation {
            inner.handled = generation;
            let _ = sender.send(Box::new(move |s: &mut AudioSessionShowState| {
                mark_command_taken(s, generation)
            }));
            match command {
                AudioSessionCommand::Start(kind) => {
                    // 旧 guard の解放失敗は旧世代の通知として捨てられないよう、現在の要求の世代で知らせる。
                    if let Some((_, old)) = inner.guard.take() {
                        release(sender, generation, old);
                    }
                    let on_signal = make_on_signal(sender.clone(), generation);
                    match session.start(to_session_kind(kind), on_signal) {
                        Ok(guard) => {
                            inner.guard = Some((generation, guard));
                            send_update(sender, generation, AudioSessionUpdate::Started);
                        }
                        Err(f) => send_update(
                            sender,
                            generation,
                            AudioSessionUpdate::Failed(to_failure(f)),
                        ),
                    }
                    if readable {
                        read_current(sender, session, generation);
                    }
                }
                AudioSessionCommand::Stop => {
                    if let Some((_, guard)) = inner.guard.take() {
                        release(sender, generation, guard);
                    }
                    if readable {
                        read_current(sender, session, generation);
                    }
                }
            }
        }
    }

    // 出来事が届いた・読み直しが押されたら、OS へ問い直す。
    if readable && (state.event_seq != inner.read_seq || state.refresh_epoch != inner.read_epoch) {
        inner.read_seq = state.event_seq;
        inner.read_epoch = state.refresh_epoch;
        read_current(sender, session, state.generation);
    }
}

/// `registry.rs` の `effect` 区画から `audio_session_show` 画面用に配線される effect ドライバ。
pub fn audio_session_show_effect(
    state: &AudioSessionShowState,
    sender: &CompletionSender<AudioSessionShowState>,
) {
    drive(
        state,
        sender,
        platform_session(),
        DRIVER.get_or_init(Driver::default),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::audio_session_show::{start, stop};
    use fandhe_device::audio_session::{
        AudioSessionEvent, CurrentState, RestoreFailed, UnsupportedAudioSession,
    };
    use fandhe_device::DeviceError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    struct Fake {
        support: Support,
        start_error: Mutex<Option<StartFailure>>,
        restore_fails: bool,
        on_signal: Mutex<Option<OnSignal>>,
        stops: Arc<AtomicUsize>,
        reads: AtomicUsize,
    }

    struct FakeGuard {
        stops: Arc<AtomicUsize>,
        restore_fails: bool,
    }

    impl AudioSessionGuard for FakeGuard {
        fn stop(self: Box<Self>) -> Result<(), RestoreFailed> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            if self.restore_fails {
                Err(RestoreFailed)
            } else {
                Ok(())
            }
        }
    }

    impl AudioSession for Fake {
        fn support(&self) -> Support {
            self.support
        }
        fn start(
            &self,
            _kind: SessionKind,
            on_signal: OnSignal,
        ) -> Result<Box<dyn AudioSessionGuard>, StartFailure> {
            if let Some(f) = self.start_error.lock().unwrap().take() {
                return Err(f);
            }
            *self.on_signal.lock().unwrap() = Some(on_signal);
            Ok(Box::new(FakeGuard {
                stops: self.stops.clone(),
                restore_fails: self.restore_fails,
            }))
        }
        fn read_current(&self) -> Result<CurrentState, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(CurrentState {
                kind: Some(ReadKind::StopOthers),
                other_audio_playing: None,
                audio_mode: None,
                music_active: None,
            })
        }
    }

    fn fake(start_error: Option<StartFailure>, restore_fails: bool) -> Arc<Fake> {
        Arc::new(Fake {
            support: Support {
                kinds: [true; 3],
                events: [true; 8],
                kind_readable: true,
                other_playing_readable: false,
                audio_mode_readable: false,
                music_active_readable: false,
            },
            start_error: Mutex::new(start_error),
            restore_fails,
            on_signal: Mutex::new(None),
            stops: Arc::new(AtomicUsize::new(0)),
            reads: AtomicUsize::new(0),
        })
    }

    type Rx = mpsc::Receiver<crate::app::Completion<AudioSessionShowState>>;

    fn channel() -> (CompletionSender<AudioSessionShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn pump(state: &mut AudioSessionShowState, rx: &Rx) {
        while let Ok(c) = rx.try_recv() {
            c(state);
        }
    }

    struct Rig {
        fake: Arc<Fake>,
        session: Arc<dyn AudioSession>,
        sender: CompletionSender<AudioSessionShowState>,
        rx: Rx,
        state: AudioSessionShowState,
        driver: Driver,
    }

    impl Rig {
        fn new(fake: Arc<Fake>) -> Self {
            let (sender, rx) = channel();
            Self {
                session: fake.clone(),
                fake,
                sender,
                rx,
                state: AudioSessionShowState::default(),
                driver: Driver::default(),
            }
        }
        fn frame(&mut self) {
            drive(&self.state, &self.sender, &self.session, &self.driver);
            pump(&mut self.state, &self.rx);
        }
        fn signal(&self, s: Signal) {
            let f = self.fake.on_signal.lock().unwrap().clone().expect("signal");
            f(s);
        }
    }

    fn focus_lost() -> Signal {
        Signal::Event(AudioSessionEvent {
            kind: EventKind::FocusLost,
            should_resume: None,
        })
    }

    #[test]
    fn start_becomes_active_and_events_reach_screen_and_trigger_read() {
        let mut r = Rig::new(fake(None, false));
        start(&mut r.state);
        r.frame();
        assert_eq!(r.state.phase, AudioSessionPhase::Active);
        assert!(r.state.support.is_some());
        let reads = r.fake.reads.load(Ordering::SeqCst);
        r.signal(focus_lost());
        pump(&mut r.state, &r.rx);
        assert_eq!(r.state.events.len(), 1);
        r.frame();
        assert!(
            r.fake.reads.load(Ordering::SeqCst) > reads,
            "出来事で読み直す"
        );
        assert!(r.state.current.is_some());
    }

    #[test]
    fn start_failures_are_never_success() {
        for f in [
            StartFailure::Unsupported,
            StartFailure::KindUnsupported,
            StartFailure::Rejected,
            StartFailure::Failed { restored: false },
        ] {
            let mut r = Rig::new(fake(Some(f), false));
            start(&mut r.state);
            r.frame();
            assert_eq!(r.state.phase, AudioSessionPhase::Failed(to_failure(f)));
            assert_eq!(r.fake.stops.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn stop_releases_once_and_late_signals_are_dropped() {
        let mut r = Rig::new(fake(None, false));
        start(&mut r.state);
        r.frame();
        stop(&mut r.state);
        r.frame();
        assert_eq!(r.fake.stops.load(Ordering::SeqCst), 1);
        r.signal(focus_lost());
        pump(&mut r.state, &r.rx);
        assert!(r.state.events.is_empty());
        r.frame();
        assert_eq!(r.fake.stops.load(Ordering::SeqCst), 1, "二重に解放しない");
    }

    #[test]
    fn restore_failure_on_stop_is_reported() {
        let mut r = Rig::new(fake(None, true));
        start(&mut r.state);
        r.frame();
        stop(&mut r.state);
        r.frame();
        assert!(r.state.restore_failed);
        assert_eq!(r.state.phase, AudioSessionPhase::Idle);
    }

    #[test]
    fn runtime_failure_releases_guard_on_next_frame() {
        let mut r = Rig::new(fake(None, false));
        start(&mut r.state);
        r.frame();
        r.signal(Signal::Failed);
        pump(&mut r.state, &r.rx);
        assert_eq!(
            r.state.phase,
            AudioSessionPhase::Failed(AudioSessionFailure::Runtime)
        );
        r.frame();
        assert_eq!(r.fake.stops.load(Ordering::SeqCst), 1);
        // 失敗後に開始し直せる
        start(&mut r.state);
        r.frame();
        assert_eq!(r.state.phase, AudioSessionPhase::Active);
    }

    #[test]
    fn restore_failure_on_restart_after_runtime_failure_is_reported() {
        let mut r = Rig::new(fake(None, true));
        start(&mut r.state);
        r.frame();
        r.signal(Signal::Failed);
        pump(&mut r.state, &r.rx);
        // Failed を反映したフレームの前に利用者が再開始する。
        start(&mut r.state);
        r.frame();
        assert_eq!(r.state.phase, AudioSessionPhase::Active);
        assert!(r.state.restore_failed, "旧 guard の復元失敗を捨てない");
    }

    #[test]
    fn double_frame_before_mark_applied_starts_once() {
        let mut r = Rig::new(fake(None, false));
        start(&mut r.state);
        drive(&r.state, &r.sender, &r.session, &r.driver);
        *r.fake.on_signal.lock().unwrap() = None;
        drive(&r.state, &r.sender, &r.session, &r.driver);
        assert!(
            r.fake.on_signal.lock().unwrap().is_none(),
            "二重に start しない"
        );
    }

    #[test]
    fn reset_releases_guard_and_lets_recreated_screen_start_again() {
        let mut r = Rig::new(fake(None, false));
        start(&mut r.state);
        r.frame();
        r.driver.reset();
        assert_eq!(r.fake.stops.load(Ordering::SeqCst), 1);
        // Activity 再生成: 画面状態だけ generation 0 に戻る。
        r.state = AudioSessionShowState::default();
        start(&mut r.state);
        r.frame();
        assert_eq!(r.state.phase, AudioSessionPhase::Active);
    }

    #[test]
    fn unsupported_os_declares_nothing_and_never_starts() {
        let mut r = Rig::new(fake(None, false));
        r.session = Arc::new(UnsupportedAudioSession);
        start(&mut r.state);
        r.frame();
        assert_eq!(
            r.state.phase,
            AudioSessionPhase::Failed(AudioSessionFailure::Unsupported)
        );
        assert!(!r.state.support.unwrap().any_kind());
    }
}
