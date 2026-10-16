//! Phase 11 の拡充ループ（親 #86）型サンプル「音声の再生」（Issue #267）の effect ドライバ。
//! `fandhe_core::screens::audio_play`（端末依存ゼロ）の要求を `take_command` で観測し、
//! `fandhe_device::media_playback::MediaPlayback` を操作して、状態を `CompletionSender` 経由で
//! 注入する（`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::media_playback`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_playback`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS: `apple_media_playback`、iOS: `ios_media_playback`、
//! Android: `android_media_playback`、Windows: `windows_media_playback`。Windows は実装済み、iOS・Android は現状スタブで
//! `UnsupportedMediaPlayback` を返す）。それ以外は `UnsupportedMediaPlayback` 固定で、ダミーの成功実装へは倒さない。
//!
//! # 並行性
//!
//! 実装はプレーヤーを 1 つだけ持つ（プロセスに 1 つ）。ドライバは `load` が返したデバイス世代と
//! 画面の要求世代を対応づけ、他の世代の snapshot は成功にしない。開始の期限は単調時計（`Instant`）で
//! 測り、期限切れ後は `stop` を呼んで遅れて始まった再生を残さず、後から届く `Playing` も捨てる。
//! 再生位置は pull 型で、画面が `Loading` / `Playing` の間だけ [`POLL_INTERVAL`] で再描画を予約する。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::audio_play::{
    apply_update, expire, mark_command_taken, take_command, AudioCommand, AudioFailure, AudioPhase,
    AudioPlayState, AudioUpdate,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::media_playback::UnsupportedMediaPlayback;
use fandhe_device::media_playback::{
    demo_wav, MediaPlayback, PlaybackFailure, PlaybackPhase, POLL_INTERVAL, START_DEADLINE,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// OS の生コードを含まない固定の失敗文言。
const FAILED_PLAY: &str = "音声の再生";
const FAILED_DECODE: &str = "音声を読み込めません";

/// プロセスに 1 つだけ持つ実装（`stop` が `play` と同じ実体に届くようにする）。
fn platform_playback() -> &'static Arc<dyn MediaPlayback> {
    static PLAYBACK: OnceLock<Arc<dyn MediaPlayback>> = OnceLock::new();
    PLAYBACK.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            super::apple_media_playback::implementation()
        }
        #[cfg(target_os = "ios")]
        {
            super::ios_media_playback::implementation()
        }
        #[cfg(target_os = "android")]
        {
            super::android_media_playback::implementation()
        }
        #[cfg(target_os = "windows")]
        {
            super::windows_media_playback::implementation()
        }
        #[cfg(not(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        )))]
        {
            Arc::new(UnsupportedMediaPlayback)
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
    /// 現在のプレーヤーのデバイス世代（`load` の戻り値）。
    device_gen: Option<u64>,
    /// 開始待ちの画面世代と起点（期限判定用）。
    pending: Option<(u64, Instant)>,
    /// 直近に snapshot を画面へ送った時刻（単調時計）。送信は即時再描画を起こすため間引く。
    last_report: Option<Instant>,
    /// 一時停止・停止を発行した時刻。非同期に確定する操作の失敗を拾うため、しばらく snapshot を監視する。
    settle_since: Option<Instant>,
    /// 一時停止・停止を発行した最初の時刻。Loading 中の窓延長に上限（期限）を設ける起点。
    settle_origin: Option<Instant>,
    /// 一時停止を発行済みで、プレーヤーが反映したか未確認のデバイス世代。反映が遅い実装（Windows の
    /// ワーカー）で Pause 直後に Play が来ても、同じ世代へ流して位置を保つための記録。
    pause_pending: Option<u64>,
}

/// 一時停止・停止の結果（非同期の失敗）を監視し続ける長さ。
const SETTLE_WINDOW: Duration = Duration::from_secs(2);

static SESSION: OnceLock<Session> = OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、ドライバの記録を破棄して再生を止める。
/// 画面状態は `default()` に戻るため、処理済み世代を残すと次の「再生」が飲み込まれる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        let mut inner = s.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(g) = inner.device_gen {
            let _ = platform_playback().stop(g);
        }
        *inner = SessionInner::default();
    }
}

fn to_failure(e: &DeviceError) -> AudioFailure {
    match e {
        DeviceError::Unsupported(_) => AudioFailure::Unsupported,
        // OS の生コードは画面へ出さない（security.md A09）。
        _ => AudioFailure::Failed(FAILED_PLAY.to_string()),
    }
}

fn snapshot_failure(f: PlaybackFailure) -> AudioFailure {
    match f {
        PlaybackFailure::Unsupported => AudioFailure::Unsupported,
        PlaybackFailure::Blocked => AudioFailure::Blocked,
        PlaybackFailure::Timeout => AudioFailure::Timeout,
        PlaybackFailure::Decode => AudioFailure::Failed(FAILED_DECODE.to_string()),
        PlaybackFailure::Internal => AudioFailure::Failed(FAILED_PLAY.to_string()),
    }
}

fn send_update(sender: &CompletionSender<AudioPlayState>, generation: u64, update: AudioUpdate) {
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut AudioPlayState| {
        apply_update(s, generation, update);
    }));
}

/// 1 フレームぶんの処理。要求の発送、snapshot の反映、期限判定を行い、
/// 次に再描画を予約した間隔（予約しなければ `None`）を返す。
fn drive(
    state: &AudioPlayState,
    sender: &CompletionSender<AudioPlayState>,
    pb: &Arc<dyn MediaPlayback>,
    session: &Session,
    deadline: Duration,
) -> Option<Duration> {
    let mut inner = session.inner.lock().ok()?;
    if let Some((command, generation)) = take_command(state) {
        if inner.handled != generation {
            inner.handled = generation;
            let _ = sender.send(Box::new(move |s: &mut AudioPlayState| {
                mark_command_taken(s, generation)
            }));
            inner.last_report = None;
            inner.settle_origin = None;
            handle_command(&mut inner, sender, pb, command, generation);
        }
    }

    let active = matches!(state.phase, AudioPhase::Loading | AudioPhase::Playing);
    if active && state.generation == inner.handled {
        if let Some((g, started)) = inner.pending {
            // 期限（単調時計）を過ぎたら、届いている Playing も成功にせず失敗で確定する。
            if g == state.generation && started.elapsed() >= deadline {
                // 開始中に背景へ回った間はフレームが回らず期限だけが進む。戻った最初のフレームでは
                // 期限より先に中断を見て、失敗（Timeout）ではなく Interrupted で確定する。
                let interrupted = inner.device_gen.is_some_and(|dg| {
                    let snap = pb.snapshot();
                    snap.generation == dg && snap.phase == PlaybackPhase::Interrupted
                });
                if interrupted {
                    report_snapshot(&mut inner, sender, pb);
                    return None;
                }
                inner.pending = None;
                let _ = sender.send(Box::new(move |s: &mut AudioPlayState| expire(s, g)));
                if let Some(dg) = inner.device_gen {
                    let _ = pb.stop(dg);
                }
                return None;
            }
        }
        // send は即時に再描画を要求するため、POLL_INTERVAL 未満では送らず連鎖を断つ。
        if inner
            .last_report
            .is_none_or(|t| t.elapsed() >= POLL_INTERVAL)
        {
            inner.last_report = Some(Instant::now());
            report_snapshot(&mut inner, sender, pb);
        }
    }

    // Paused / Stopped でも、操作直後は OS 側の失敗が後から確定しうるので失敗だけ拾う。
    // 画面側（apply_update）は Paused / Stopped への Failed を受け付ける。
    // 読み込み処理中（プレーヤー側が Loading）は操作が未実行のままなので、窓の起点を進め続ける。
    // 読み込み完了後の SETTLE_WINDOW で、キュー済みの一時停止・停止の失敗を拾う。
    let awaiting = !active
        && matches!(state.phase, AudioPhase::Paused | AudioPhase::Stopped)
        && state.generation == inner.handled
        && inner.settle_since.is_some();
    if awaiting
        && inner.device_gen.is_some_and(|dg| {
            let snap = pb.snapshot();
            // Loading は操作が未実行、Playing は一時停止・停止が worker で未完了（Windows）。
            snap.generation == dg
                && matches!(snap.phase, PlaybackPhase::Loading | PlaybackPhase::Playing)
        })
        // CANPLAY が来ない読み込みで再描画を出し続けないよう、延長は期限までに限る。
        && inner.settle_origin.is_some_and(|t| t.elapsed() < deadline)
    {
        inner.settle_since = Some(Instant::now());
    }
    let settling = awaiting
        && inner
            .settle_since
            .is_some_and(|t| t.elapsed() < SETTLE_WINDOW);
    // 操作が未完了のうちに届く Playing は旧状態の残りなので画面へ反映しない（失敗だけ拾う）。
    let in_flight = inner.device_gen.is_some_and(|dg| {
        let snap = pb.snapshot();
        snap.generation == dg && snap.phase == PlaybackPhase::Playing
    });
    if settling
        && !in_flight
        && inner
            .last_report
            .is_none_or(|t| t.elapsed() >= POLL_INTERVAL)
    {
        inner.last_report = Some(Instant::now());
        report_snapshot(&mut inner, sender, pb);
    }

    if active || settling {
        sender.request_repaint_after(POLL_INTERVAL);
        Some(POLL_INTERVAL)
    } else {
        None
    }
}

fn handle_command(
    inner: &mut SessionInner,
    sender: &CompletionSender<AudioPlayState>,
    pb: &Arc<dyn MediaPlayback>,
    command: AudioCommand,
    generation: u64,
) {
    let fail = |inner: &mut SessionInner, e: &DeviceError| {
        inner.pending = None;
        send_update(sender, generation, AudioUpdate::Failed(to_failure(e)));
    };
    match command {
        AudioCommand::Play => {
            // 期限の起点は同期の load・play より前に取る（その所要時間も期限に含める）。
            let started = Instant::now();
            let snap = pb.snapshot();
            // 一時停止の反映待ち（snapshot がまだ Playing）でも同じ世代を再開し、再 load で位置を失わない。
            let pause_in_flight =
                inner.pause_pending.is_some() && inner.pause_pending == inner.device_gen;
            let reusable = inner.device_gen.is_some_and(|g| {
                snap.generation == g
                    && (matches!(snap.phase, PlaybackPhase::Ready | PlaybackPhase::Paused)
                        || (pause_in_flight && snap.phase == PlaybackPhase::Playing))
            });
            inner.pause_pending = None;
            if !reusable {
                match pb.load(demo_wav()) {
                    Ok(g) => inner.device_gen = Some(g),
                    Err(e) => {
                        inner.device_gen = None;
                        return fail(inner, &e);
                    }
                }
            }
            let Some(g) = inner.device_gen else { return };
            match pb.play(g) {
                Ok(()) => inner.pending = Some((generation, started)),
                Err(e) => fail(inner, &e),
            }
        }
        AudioCommand::Pause | AudioCommand::Stop => {
            inner.pending = None;
            // プレーヤーが無ければ止める対象も無い（画面側の遷移だけで足りる）。
            let Some(g) = inner.device_gen else { return };
            inner.settle_since = Some(Instant::now());
            inner.settle_origin.get_or_insert_with(Instant::now);
            inner.pause_pending = (command == AudioCommand::Pause).then_some(g);
            let r = if command == AudioCommand::Pause {
                pb.pause(g)
            } else {
                pb.stop(g)
            };
            if let Err(e) = r {
                fail(inner, &e);
            }
        }
    }
}

/// 現在のプレーヤーの snapshot を、世代が一致するときだけ画面へ送る。
fn report_snapshot(
    inner: &mut SessionInner,
    sender: &CompletionSender<AudioPlayState>,
    pb: &Arc<dyn MediaPlayback>,
) {
    let Some(dg) = inner.device_gen else { return };
    let snap = pb.snapshot();
    if snap.generation != dg {
        return;
    }
    let generation = inner.handled;
    let phase = match snap.phase {
        PlaybackPhase::Playing => AudioPhase::Playing,
        PlaybackPhase::Ended => AudioPhase::Ended,
        PlaybackPhase::Interrupted => AudioPhase::Interrupted,
        PlaybackPhase::Failed(f) => {
            inner.pending = None;
            return send_update(sender, generation, AudioUpdate::Failed(snapshot_failure(f)));
        }
        // 読み込み中・一時停止・停止は利用者の操作が画面側の正で、ここでは反映しない。
        _ => return,
    };
    inner.pending = None;
    send_update(
        sender,
        generation,
        AudioUpdate::Snapshot {
            phase,
            position_ms: snap.position_ms,
            duration_ms: snap.duration_ms,
        },
    );
}

/// `registry.rs` の `effect` 区画から `audio_play` 画面用に配線される effect ドライバ。
pub fn audio_play_effect(state: &AudioPlayState, sender: &CompletionSender<AudioPlayState>) {
    drive(
        state,
        sender,
        platform_playback(),
        SESSION.get_or_init(Session::default),
        START_DEADLINE,
    );
}

/// アプリ終了時にプレーヤーを解放する（`device::shutdown` から呼ばれる）。
pub(super) fn shutdown() {
    platform_playback().shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::audio_play::{pause, play, stop};
    use fandhe_device::media_playback::PlaybackSnapshot;
    use std::sync::mpsc;

    struct FakeSt {
        gen: u64,
        phase: PlaybackPhase,
        load_err: Option<DeviceError>,
        play_err: Option<DeviceError>,
        auto_play: bool,
        loads: usize,
        stops: usize,
    }

    struct Fake(Mutex<FakeSt>);

    impl Fake {
        fn new(auto_play: bool) -> Arc<Self> {
            Arc::new(Self(Mutex::new(FakeSt {
                gen: 0,
                phase: PlaybackPhase::Idle,
                load_err: None,
                play_err: None,
                auto_play,
                loads: 0,
                stops: 0,
            })))
        }
        fn set_phase(&self, p: PlaybackPhase) {
            self.0.lock().unwrap().phase = p;
        }
    }

    impl MediaPlayback for Fake {
        fn load(&self, _wav: Arc<[u8]>) -> Result<u64, DeviceError> {
            let mut s = self.0.lock().unwrap();
            if let Some(e) = s.load_err.take() {
                return Err(e);
            }
            s.gen += 1;
            s.loads += 1;
            s.phase = PlaybackPhase::Ready;
            Ok(s.gen)
        }
        fn play(&self, g: u64) -> Result<(), DeviceError> {
            let mut s = self.0.lock().unwrap();
            if g != s.gen {
                return Err(DeviceError::InvalidInput("media_playback generation"));
            }
            if let Some(e) = s.play_err.take() {
                return Err(e);
            }
            if s.auto_play {
                s.phase = PlaybackPhase::Playing;
            }
            Ok(())
        }
        fn pause(&self, _g: u64) -> Result<(), DeviceError> {
            self.0.lock().unwrap().phase = PlaybackPhase::Paused;
            Ok(())
        }
        fn stop(&self, _g: u64) -> Result<(), DeviceError> {
            let mut s = self.0.lock().unwrap();
            s.stops += 1;
            s.phase = PlaybackPhase::Ready;
            Ok(())
        }
        fn snapshot(&self) -> PlaybackSnapshot {
            let s = self.0.lock().unwrap();
            PlaybackSnapshot {
                generation: s.gen,
                phase: s.phase,
                position_ms: Some(1000),
                duration_ms: Some(5000),
            }
        }
        fn shutdown(&self) {}
    }

    type Rx = mpsc::Receiver<crate::app::Completion<AudioPlayState>>;

    struct H {
        fake: Arc<Fake>,
        pb: Arc<dyn MediaPlayback>,
        sender: CompletionSender<AudioPlayState>,
        rx: Rx,
        session: Session,
        state: AudioPlayState,
    }

    impl H {
        fn new(fake: Arc<Fake>) -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                pb: fake.clone(),
                fake,
                sender: CompletionSender::new(tx, egui::Context::default()),
                rx,
                session: Session::default(),
                state: AudioPlayState::default(),
            }
        }
        /// 間引きを無効にして 1 フレーム進める。
        fn drive(&mut self, d: Duration) -> Option<Duration> {
            self.session.inner.lock().unwrap().last_report = None;
            self.drive_throttled(d)
        }
        fn drive_throttled(&mut self, d: Duration) -> Option<Duration> {
            let r = drive(&self.state, &self.sender, &self.pb, &self.session, d);
            while let Ok(c) = self.rx.try_recv() {
                c(&mut self.state);
            }
            r
        }
    }

    const D: Duration = Duration::from_secs(5);

    #[test]
    fn sync_play_becomes_playing_and_polls() {
        let mut h = H::new(Fake::new(true));
        play(&mut h.state);
        assert_eq!(h.drive(D), Some(POLL_INTERVAL));
        assert_eq!(h.state.phase, AudioPhase::Playing);
        assert_eq!(take_command(&h.state), None);
        // 位置が画面へ届く
        assert_eq!(h.state.position_ms, Some(1000));
    }

    #[test]
    fn snapshot_reports_are_throttled() {
        let mut h = H::new(Fake::new(true));
        play(&mut h.state);
        h.drive(D);
        h.fake.set_phase(PlaybackPhase::Ended);
        h.drive_throttled(D);
        assert_eq!(h.state.phase, AudioPhase::Playing, "間隔内は送らない");
    }

    #[test]
    fn slow_start_counts_from_before_load() {
        struct Slow(Arc<Fake>);
        impl MediaPlayback for Slow {
            fn load(&self, w: Arc<[u8]>) -> Result<u64, DeviceError> {
                std::thread::sleep(Duration::from_millis(40));
                self.0.load(w)
            }
            fn play(&self, g: u64) -> Result<(), DeviceError> {
                self.0.play(g)
            }
            fn pause(&self, g: u64) -> Result<(), DeviceError> {
                self.0.pause(g)
            }
            fn stop(&self, g: u64) -> Result<(), DeviceError> {
                self.0.stop(g)
            }
            fn snapshot(&self) -> PlaybackSnapshot {
                self.0.snapshot()
            }
            fn shutdown(&self) {}
        }
        let mut h = H::new(Fake::new(true));
        h.pb = Arc::new(Slow(h.fake.clone()));
        play(&mut h.state);
        h.drive(Duration::from_millis(20));
        assert_eq!(h.state.phase, AudioPhase::Failed(AudioFailure::Timeout));
        assert_eq!(h.fake.0.lock().unwrap().stops, 1);
    }

    #[test]
    fn load_and_play_errors_are_never_success() {
        for (load_err, play_err, unsupported) in [
            (Some(DeviceError::Unsupported("media_playback")), None, true),
            (Some(DeviceError::Internal("x")), None, false),
            (None, Some(DeviceError::Internal("x")), false),
        ] {
            let fake = Fake::new(true);
            {
                let mut s = fake.0.lock().unwrap();
                s.load_err = load_err;
                s.play_err = play_err;
            }
            let mut h = H::new(fake);
            play(&mut h.state);
            h.drive(D);
            match &h.state.phase {
                AudioPhase::Failed(AudioFailure::Unsupported) => assert!(unsupported),
                AudioPhase::Failed(AudioFailure::Failed(_)) => assert!(!unsupported),
                p => panic!("{p:?}"),
            }
        }
    }

    #[test]
    fn deadline_fails_stops_and_drops_late_playing() {
        let mut h = H::new(Fake::new(false));
        play(&mut h.state);
        let d = Duration::from_millis(20);
        h.drive(d);
        assert_eq!(h.state.phase, AudioPhase::Loading);
        std::thread::sleep(Duration::from_millis(40));
        // 期限超過後に Playing が確定していても成功にしない。
        h.fake.set_phase(PlaybackPhase::Playing);
        assert_eq!(h.drive(d), None);
        assert_eq!(h.state.phase, AudioPhase::Failed(AudioFailure::Timeout));
        assert_eq!(h.fake.0.lock().unwrap().stops, 1);
        h.fake.set_phase(PlaybackPhase::Playing);
        h.drive(d);
        assert_eq!(h.state.phase, AudioPhase::Failed(AudioFailure::Timeout));
    }

    #[test]
    fn interrupt_before_deadline_stays_interrupted_after_deadline() {
        let mut h = H::new(Fake::new(false));
        play(&mut h.state);
        let d = Duration::from_millis(20);
        h.drive(d);
        assert_eq!(h.state.phase, AudioPhase::Loading);
        // 背景中はフレームが回らず、その間に期限が過ぎる。
        h.fake.set_phase(PlaybackPhase::Interrupted);
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(h.drive(d), None);
        assert_eq!(h.state.phase, AudioPhase::Interrupted);
        assert_eq!(h.fake.0.lock().unwrap().stops, 0);
    }

    #[test]
    fn stale_device_generation_snapshot_is_dropped() {
        let mut h = H::new(Fake::new(false));
        play(&mut h.state);
        h.drive(D);
        h.fake.0.lock().unwrap().gen += 5; // 別世代のプレーヤーの Playing
        h.fake.set_phase(PlaybackPhase::Playing);
        h.drive(D);
        assert_eq!(h.state.phase, AudioPhase::Loading);
    }

    #[test]
    fn ended_and_decode_failure_are_distinguished() {
        let mut h = H::new(Fake::new(true));
        play(&mut h.state);
        h.drive(D);
        h.fake.set_phase(PlaybackPhase::Ended);
        h.drive(D);
        assert_eq!(h.state.phase, AudioPhase::Ended);

        let mut h = H::new(Fake::new(true));
        play(&mut h.state);
        h.drive(D);
        h.fake
            .set_phase(PlaybackPhase::Failed(PlaybackFailure::Decode));
        h.drive(D);
        assert!(matches!(
            h.state.phase,
            AudioPhase::Failed(AudioFailure::Failed(_))
        ));
    }

    #[test]
    fn pause_and_stop_are_not_failures_and_stop_polling() {
        let mut h = H::new(Fake::new(true));
        play(&mut h.state);
        h.drive(D);
        pause(&mut h.state);
        assert_eq!(
            h.drive(D),
            Some(POLL_INTERVAL),
            "操作直後は失敗確定を監視する"
        );
        assert_eq!(h.state.phase, AudioPhase::Paused);
        play(&mut h.state);
        h.drive(D);
        assert_eq!(
            h.fake.0.lock().unwrap().loads,
            1,
            "一時停止からは再 load しない"
        );
        stop(&mut h.state);
        assert_eq!(h.drive(D), Some(POLL_INTERVAL));
        assert_eq!(h.state.phase, AudioPhase::Stopped);
    }

    /// pause の反映が遅い実装（Windows のワーカー）。反映までは snapshot が Playing のまま。
    struct LatePause(Arc<Fake>);
    impl MediaPlayback for LatePause {
        fn load(&self, w: Arc<[u8]>) -> Result<u64, DeviceError> {
            self.0.load(w)
        }
        fn play(&self, g: u64) -> Result<(), DeviceError> {
            self.0.play(g)
        }
        fn pause(&self, _g: u64) -> Result<(), DeviceError> {
            Ok(())
        }
        fn stop(&self, g: u64) -> Result<(), DeviceError> {
            self.0.stop(g)
        }
        fn snapshot(&self) -> PlaybackSnapshot {
            self.0.snapshot()
        }
        fn shutdown(&self) {}
    }

    #[test]
    fn play_during_unapplied_pause_resumes_same_generation() {
        let mut h = H::new(Fake::new(true));
        h.pb = Arc::new(LatePause(h.fake.clone()));
        play(&mut h.state);
        h.drive(D);
        assert_eq!(h.state.phase, AudioPhase::Playing);
        pause(&mut h.state);
        h.drive(D); // pause は未反映（snapshot は Playing）
        play(&mut h.state);
        h.drive(D);
        assert_eq!(h.fake.0.lock().unwrap().loads, 1, "再 load しない");
        assert_eq!(h.state.phase, AudioPhase::Playing);
    }

    #[test]
    fn async_pause_failure_reaches_screen() {
        let mut h = H::new(Fake::new(true));
        play(&mut h.state);
        h.drive(D);
        pause(&mut h.state);
        h.drive(D);
        assert_eq!(h.state.phase, AudioPhase::Paused);
        h.fake
            .set_phase(PlaybackPhase::Failed(PlaybackFailure::Internal));
        h.drive(D);
        assert!(matches!(h.state.phase, AudioPhase::Failed(_)));
    }

    #[test]
    fn play_after_ended_reloads() {
        let mut h = H::new(Fake::new(true));
        play(&mut h.state);
        h.drive(D);
        h.fake.set_phase(PlaybackPhase::Ended);
        h.drive(D);
        play(&mut h.state);
        h.drive(D);
        assert_eq!(h.fake.0.lock().unwrap().loads, 2);
        assert_eq!(h.state.phase, AudioPhase::Playing);
    }

    #[test]
    fn failed_does_not_poll() {
        let fake = Fake::new(true);
        fake.0.lock().unwrap().load_err = Some(DeviceError::Internal("x"));
        let mut h = H::new(fake);
        play(&mut h.state);
        h.drive(D);
        assert_eq!(h.drive(D), None);
    }

    #[test]
    fn reset_lets_recreated_screen_play_again() {
        let mut h = H::new(Fake::new(true));
        play(&mut h.state);
        h.drive(D);
        // Activity 再生成: 画面状態だけ generation 0 に戻る。
        h.state = AudioPlayState::default();
        play(&mut h.state);
        h.fake.set_phase(PlaybackPhase::Ended);
        h.drive(D);
        assert_eq!(
            h.fake.0.lock().unwrap().loads,
            1,
            "リセット無しでは飲み込まれる"
        );
        *h.session.inner.lock().unwrap() = SessionInner::default();
        h.drive(D);
        assert_eq!(h.fake.0.lock().unwrap().loads, 2);
    }

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert!(matches!(
            platform_playback().load(demo_wav()),
            Err(DeviceError::Unsupported("media_playback"))
        ));
    }
}
