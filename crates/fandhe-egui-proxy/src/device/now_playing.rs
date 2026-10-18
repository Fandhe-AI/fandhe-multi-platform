//! Phase 11 の拡充ループ（親 #98、Issue #381）「ロック画面・OS のメディア操作」の effect ドライバ。
//! `fandhe_core::screens::audio_play`（端末依存ゼロ）が `Playing` / `Paused` の間だけ、
//! `fandhe_device::now_playing::NowPlaying` で曲名と再生状態を OS の「再生中」表示へ渡し、
//! OS から来た再生・一時停止を画面の状態へ返す。`media_playback::audio_play_effect` の最後から
//! [`sync_audio_play`] が毎フレーム同期に呼ばれる（registry は触らない）。契約の SSOT は
//! `fandhe_device::now_playing`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_now_playing`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS: `apple_now_playing`、iOS: `ios_now_playing`（#382）、
//! Android: `android_now_playing`（#383）、Windows: `windows_now_playing`（#385）。macOS 以外は現状スタブで
//! `UnsupportedNowPlaying` を返す）。それ以外は `UnsupportedNowPlaying` 固定で、ダミーの成功実装へは倒さない。
//!
//! # 並行性
//!
//! OS 側のコールバックは [`commands`] の共有状態へ `push` するだけ。このドライバが毎フレーム `take` して
//! 画面へ `Completion` で返す。取り下げでは **`take` より先に** `disarm` するので、停止を確定した後に
//! 届いた古い操作では再開しない。`Loading`（再開の確定待ち）の間は操作を取り出さず残す。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use fandhe_core::screens::audio_play::{
    apply_os_command, os_media_cleared, set_os_media_status, AudioPhase, AudioPlayState, OsCommand,
    OsMediaStatus,
};
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
use fandhe_device::now_playing::UnsupportedNowPlaying;
use fandhe_device::now_playing::{NowPlaying, NowPlayingInfo, OsMediaCommand, SharedOsCommand};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// OS からの操作を拾う再描画の間隔（macOS・Windows。入力が無くても egui が止まらないようにする）。
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// プロセスに 1 つだけ持つ実装（`clear` / `shutdown` が `publish` と同じ実体に届くようにする）。
fn platform_now_playing() -> &'static Arc<dyn NowPlaying> {
    static INSTANCE: OnceLock<Arc<dyn NowPlaying>> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            super::apple_now_playing::implementation()
        }
        #[cfg(target_os = "ios")]
        {
            super::ios_now_playing::implementation()
        }
        #[cfg(target_os = "android")]
        {
            super::android_now_playing::implementation()
        }
        #[cfg(target_os = "windows")]
        {
            super::windows_now_playing::implementation()
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "android",
            target_os = "windows"
        )))]
        {
            Arc::new(UnsupportedNowPlaying)
        }
    })
}

/// プロセスに 1 つの共有状態。各 OS の層は `push` だけを呼ぶ。
pub(crate) fn commands() -> &'static SharedOsCommand {
    static COMMANDS: OnceLock<SharedOsCommand> = OnceLock::new();
    COMMANDS.get_or_init(SharedOsCommand::new)
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
#[derive(Default)]
struct Session {
    /// OS へ渡している状態（`Some(playing)`）。`publish` の失敗でも記録し、毎フレーム呼び直さない。
    shown: Mutex<Option<bool>>,
}

static SESSION: OnceLock<Session> = OnceLock::new();

fn to_status(e: &DeviceError) -> OsMediaStatus {
    match e {
        DeviceError::Unsupported(_) => OsMediaStatus::Unsupported,
        // OS の生コードは画面へ出さない（security.md A09）。
        _ => OsMediaStatus::Failed,
    }
}

fn to_screen_command(c: OsMediaCommand) -> OsCommand {
    match c {
        OsMediaCommand::Play => OsCommand::Play,
        OsMediaCommand::Pause => OsCommand::Pause,
        OsMediaCommand::Toggle => OsCommand::Toggle,
    }
}

/// 1 フレームぶんの同期（本体。テストから実装・共有状態・Session を差し込めるよう切り出す）。
fn sync(
    state: &AudioPlayState,
    sender: &CompletionSender<AudioPlayState>,
    np: &dyn NowPlaying,
    cmds: &SharedOsCommand,
    session: &Session,
) {
    let mut shown = session.shown.lock().unwrap_or_else(|p| p.into_inner());
    let playing = match state.phase {
        AudioPhase::Playing => true,
        AudioPhase::Paused => false,
        // 再開の確定待ち。OS の表示も保留中の操作もそのまま残す。
        AudioPhase::Loading => return,
        _ => {
            if shown.take().is_some() {
                // 先に受付を止めて保留中の操作を捨て、停止後の古い操作で再開させない。
                cmds.disarm();
                np.clear();
                let _ = sender.send(Box::new(os_media_cleared));
            }
            return;
        }
    };

    // 再開要求（Play・一時停止からの Toggle）を送ったらそこで取り出しを止め、後続は次の
    // Loading 明けまで残す（Loading 中の apply_os_command に後続を捨てさせない）。
    let mut playing_now = playing;
    while let Some(cmd) = cmds.take() {
        let resumes = match cmd {
            OsMediaCommand::Play | OsMediaCommand::Toggle => !playing_now,
            OsMediaCommand::Pause => false,
        };
        match cmd {
            OsMediaCommand::Pause => playing_now = false,
            OsMediaCommand::Toggle if playing_now => playing_now = false,
            _ => {}
        }
        let cmd = to_screen_command(cmd);
        let _ = sender.send(Box::new(move |s: &mut AudioPlayState| {
            apply_os_command(s, cmd)
        }));
        if resumes {
            break;
        }
    }

    if *shown != Some(playing) {
        *shown = Some(playing);
        cmds.arm();
        let info = NowPlayingInfo::new(playing, state.position_ms, state.duration_ms);
        let status = match np.publish(info) {
            Ok(()) => OsMediaStatus::Sent,
            Err(e) => {
                cmds.disarm();
                to_status(&e)
            }
        };
        let _ = sender.send(Box::new(move |s: &mut AudioPlayState| {
            set_os_media_status(s, status)
        }));
    }

    // ponytail: 一時停止中も 4Hz で再描画する。下げるなら OS 側から ctx を起こす口を足す。
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        sender.request_repaint_after(POLL_INTERVAL);
    }
}

/// `media_playback::audio_play_effect` から毎フレーム呼ばれる。
pub(super) fn sync_audio_play(state: &AudioPlayState, sender: &CompletionSender<AudioPlayState>) {
    sync(
        state,
        sender,
        platform_now_playing().as_ref(),
        commands(),
        SESSION.get_or_init(Session::default),
    );
}

/// Activity 再生成時に記録を捨てて表示を取り下げる（画面状態は `default()` に戻るため、残すと
/// 次の再生で表示が出ない）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    commands().disarm();
    platform_now_playing().clear();
    if let Some(s) = SESSION.get() {
        *s.shown.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// アプリ終了時に表示を取り下げる（`device::shutdown` から呼ばれる）。
pub(super) fn shutdown() {
    commands().disarm();
    platform_now_playing().shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::audio_play::{apply_update, play, stop, AudioUpdate, OsCommandSeen};
    use std::sync::mpsc;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        publishes: Mutex<Vec<bool>>,
        clears: Mutex<usize>,
        err: Mutex<Option<DeviceError>>,
    }

    impl NowPlaying for Fake {
        fn publish(&self, info: NowPlayingInfo) -> Result<(), DeviceError> {
            self.publishes.lock().unwrap().push(info.playing);
            match self.err.lock().unwrap().take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
        fn clear(&self) {
            *self.clears.lock().unwrap() += 1;
        }
        fn shutdown(&self) {}
    }

    struct H {
        fake: Fake,
        cmds: SharedOsCommand,
        session: Session,
        sender: CompletionSender<AudioPlayState>,
        rx: mpsc::Receiver<crate::app::Completion<AudioPlayState>>,
        state: AudioPlayState,
    }

    impl H {
        fn new() -> Self {
            let (tx, rx) = mpsc::channel();
            let mut state = AudioPlayState::default();
            play(&mut state);
            let g = state.generation;
            apply_update(
                &mut state,
                g,
                AudioUpdate::Snapshot {
                    phase: AudioPhase::Playing,
                    position_ms: Some(0),
                    duration_ms: Some(5000),
                },
            );
            Self {
                fake: Fake::default(),
                cmds: SharedOsCommand::new(),
                session: Session::default(),
                sender: CompletionSender::new(tx, egui::Context::default()),
                rx,
                state,
            }
        }
        /// 1 フレーム: 同期して、届いた Completion を画面へ反映する。
        fn frame(&mut self) {
            sync(
                &self.state,
                &self.sender,
                &self.fake,
                &self.cmds,
                &self.session,
            );
            while let Ok(c) = self.rx.try_recv() {
                c(&mut self.state);
            }
        }
    }

    #[test]
    fn publishes_once_and_again_on_pause() {
        let mut h = H::new();
        h.frame();
        h.frame();
        assert_eq!(*h.fake.publishes.lock().unwrap(), vec![true]);
        assert_eq!(h.state.os_media, OsMediaStatus::Sent);
        h.cmds.push(OsMediaCommand::Pause);
        h.frame();
        assert_eq!(h.state.phase, AudioPhase::Paused);
        h.frame();
        assert_eq!(*h.fake.publishes.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn os_pause_is_applied_without_failure() {
        let mut h = H::new();
        h.frame();
        h.cmds.push(OsMediaCommand::Pause);
        h.frame();
        assert_eq!(h.state.phase, AudioPhase::Paused);
        assert_eq!(h.state.last_os_command, Some(OsCommandSeen::Pause));
        assert_eq!(fandhe_core::screens::audio_play::error_text(&h.state), "");
    }

    #[test]
    fn stale_command_after_stop_does_not_resume() {
        let mut h = H::new();
        h.frame();
        stop(&mut h.state);
        // 停止を確定した後に届いた古い再生操作。
        h.cmds.push(OsMediaCommand::Play);
        h.frame();
        assert_eq!(*h.fake.clears.lock().unwrap(), 1);
        assert_eq!(h.cmds.take(), None);
        assert!(!h.cmds.push(OsMediaCommand::Play));
        assert_eq!(h.state.phase, AudioPhase::Stopped);
        assert_eq!(h.state.os_media, OsMediaStatus::Off);
    }

    #[test]
    fn loading_keeps_pending_command_until_playing() {
        let mut h = H::new();
        h.frame();
        h.cmds.push(OsMediaCommand::Pause);
        h.frame();
        assert_eq!(h.state.phase, AudioPhase::Paused);
        // 再開の確定待ちの間に届いた操作は取り出さず残す。
        play(&mut h.state);
        h.cmds.push(OsMediaCommand::Pause);
        h.frame();
        assert_eq!(h.cmds.take(), Some(OsMediaCommand::Pause));
        h.cmds.push(OsMediaCommand::Pause);
        let g = h.state.generation;
        apply_update(
            &mut h.state,
            g,
            AudioUpdate::Snapshot {
                phase: AudioPhase::Playing,
                position_ms: Some(0),
                duration_ms: Some(5000),
            },
        );
        h.frame();
        assert_eq!(h.state.phase, AudioPhase::Paused);
    }

    #[test]
    fn play_then_pause_in_one_batch_keeps_pause_after_loading() {
        let mut h = H::new();
        h.frame();
        h.cmds.push(OsMediaCommand::Pause);
        h.frame();
        assert_eq!(h.state.phase, AudioPhase::Paused);
        h.cmds.push(OsMediaCommand::Play);
        h.cmds.push(OsMediaCommand::Pause);
        h.frame();
        assert_eq!(h.state.phase, AudioPhase::Loading);
        assert_eq!(h.cmds.take(), Some(OsMediaCommand::Pause));
        h.cmds.push(OsMediaCommand::Pause);
        let g = h.state.generation;
        apply_update(
            &mut h.state,
            g,
            AudioUpdate::Snapshot {
                phase: AudioPhase::Playing,
                position_ms: Some(0),
                duration_ms: Some(5000),
            },
        );
        h.frame();
        assert_eq!(h.state.phase, AudioPhase::Paused);
    }

    #[test]
    fn publish_failure_is_not_success_and_rejects_pushes() {
        for (err, want) in [
            (DeviceError::Internal("now_playing"), OsMediaStatus::Failed),
            (
                DeviceError::Unsupported("now_playing"),
                OsMediaStatus::Unsupported,
            ),
        ] {
            let mut h = H::new();
            *h.fake.err.lock().unwrap() = Some(err);
            h.frame();
            assert_eq!(h.state.os_media, want);
            assert!(!h.cmds.push(OsMediaCommand::Play));
            h.frame();
            // 毎フレーム呼び直さない。
            assert_eq!(h.fake.publishes.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn unsupported_dispatch_never_succeeds() {
        let mut h = H::new();
        let np = fandhe_device::now_playing::UnsupportedNowPlaying;
        sync(&h.state, &h.sender, &np, &h.cmds, &h.session);
        while let Ok(c) = h.rx.try_recv() {
            c(&mut h.state);
        }
        assert_eq!(h.state.os_media, OsMediaStatus::Unsupported);
    }
}
