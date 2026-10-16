//! Phase 11（端末機能の拡充ループ、親 #86）型サンプル「音声の再生」（Issue #267）。
//! 「再生」「一時停止」「停止」の 3 ボタンで、同梱の短い音声 1 本を鳴らす 1 画面。
//! `FANDHE_PROXY_APP=audio_play` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod audio_play;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/media_playback.rs` が [`take_command`] で要求を観測し、
//! [`apply_update`] で通知を注入する（`midi_show.rs` と同型の状態機械）。
//!
//! 全対象 OS（macOS / iOS / Android / Windows / Web）の状態・文言をここで確定済みで、後続の OS 実装は
//! 本画面を触らない。ボタンは Web の `mount` が `Screen` を作り直さない制約のため状態によらず常に出す。
//!
//! ```text
//! 再生: Idle/Paused/Stopped/Ended/Interrupted/Failed --> Loading（generation += 1, command = Play）
//!   --Snapshot(Playing)--> Playing --Snapshot(Ended)--> Ended
//! Playing --「一時停止」--> Paused（command = Pause）   * --「停止」--> Stopped（command = Stop）
//! Loading --期限切れ expire--> Failed(Timeout)
//! ```
//!
//! 利用者の一時停止・停止は失敗ではない。`Interrupted`（OS による中断）も失敗ではなく、復帰しても
//! 自動では再開しない。世代の違う通知と、`Loading` / `Playing` 以外で届いた通知は捨てる。

use crate::{Screen, WidgetId};

const PLAY_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const PAUSE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 画面タイトル。
pub const TITLE: &str = "音声の再生";
/// 再生ボタンのラベル。
pub const PLAY_LABEL: &str = "再生";
/// 一時停止ボタンのラベル。
pub const PAUSE_LABEL: &str = "一時停止";
/// 停止ボタンのラベル。
pub const STOP_LABEL: &str = "停止";
/// 読み込み中の案内。
pub const LOADING_STATUS: &str = "読み込んでいます…";
/// 再生中の案内。
pub const PLAYING_STATUS: &str = "再生中";
/// 一時停止中の案内。
pub const PAUSED_STATUS: &str = "一時停止中";
/// 最後まで鳴ったときの案内（失敗ではない）。
pub const ENDED_STATUS: &str = "再生し終えました";
/// 停止後の案内。
pub const STOPPED_STATUS: &str = "停止しました";
/// 中断時の案内（失敗ではない）。
pub const INTERRUPTED_STATUS: &str = "中断しました（もう一度押すと再生します）";
/// 自動再生をブロックされたときの案内（Web）。
pub const BLOCKED_STATUS: &str = "ブラウザに再生を止められました。もう一度押してください";
/// 期限切れの案内。
pub const TIMEOUT_STATUS: &str = "再生を始められませんでした（期限切れ）";
/// 非対応のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では音声を再生できません";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 位置・長さが未取得のときの表示。
pub const UNKNOWN_TIME: &str = "--:--";

/// 画面側の進行状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AudioPhase {
    /// 未再生。
    #[default]
    Idle,
    /// 読み込み・再生開始の確定待ち。
    Loading,
    /// 再生中。
    Playing,
    /// 一時停止中。
    Paused,
    /// 停止済み。
    Stopped,
    /// 最後まで鳴った。
    Ended,
    /// OS による中断。
    Interrupted,
    /// 失敗。
    Failed(AudioFailure),
}

/// 失敗の種別（`fandhe_device` の型から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioFailure {
    /// 非対応。
    Unsupported,
    /// 自動再生のブロック（Web）。
    Blocked,
    /// 期限切れ。
    Timeout,
    /// それ以外（固定文言のみ。OS の生コードを入れない）。
    Failed(String),
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCommand {
    /// 再生する（必要なら読み込み直す）。
    Play,
    /// 一時停止する。
    Pause,
    /// 停止して先頭へ戻す。
    Stop,
}

/// effect ドライバから注入する通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioUpdate {
    /// 状態と位置・長さ（ミリ秒、未取得は `None`）。`Playing` / `Ended` / `Interrupted` のみ意味を持つ。
    Snapshot {
        /// 新しい状態。
        phase: AudioPhase,
        /// 再生位置。
        position_ms: Option<u64>,
        /// 長さ。
        duration_ms: Option<u64>,
    },
    /// 失敗。
    Failed(AudioFailure),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioPlayState {
    /// 進行状態。
    pub phase: AudioPhase,
    /// 直近に立てた要求の世代。
    pub generation: u64,
    /// 未発送の要求。
    pub command: Option<AudioCommand>,
    /// 再生位置（ミリ秒）。
    pub position_ms: Option<u64>,
    /// 長さ（ミリ秒）。
    pub duration_ms: Option<u64>,
}

fn issue(state: &mut AudioPlayState, command: AudioCommand) {
    state.generation = state.generation.wrapping_add(1);
    state.command = Some(command);
}

/// 「再生」。`Loading` / `Playing` 中は受け付けない。
pub fn play(state: &mut AudioPlayState) {
    if matches!(state.phase, AudioPhase::Loading | AudioPhase::Playing) {
        return;
    }
    issue(state, AudioCommand::Play);
    state.phase = AudioPhase::Loading;
}

/// 「一時停止」。`Playing` のときだけ受け付ける。失敗ではなく `Paused`。
pub fn pause(state: &mut AudioPlayState) {
    if state.phase != AudioPhase::Playing {
        return;
    }
    issue(state, AudioCommand::Pause);
    state.phase = AudioPhase::Paused;
}

/// 「停止」。再生・読み込み・一時停止のときだけ受け付ける。失敗ではなく `Stopped`。
pub fn stop(state: &mut AudioPlayState) {
    if !matches!(
        state.phase,
        AudioPhase::Loading | AudioPhase::Playing | AudioPhase::Paused
    ) {
        return;
    }
    issue(state, AudioCommand::Stop);
    state.phase = AudioPhase::Stopped;
    state.position_ms = Some(0);
}

/// 未発送の要求と世代を観測する getter（状態は変えない）。
pub fn take_command(state: &AudioPlayState) -> Option<(AudioCommand, u64)> {
    state.command.map(|c| (c, state.generation))
}

/// 要求を発送済みにする。世代が違えば何もしない。
pub fn mark_command_taken(state: &mut AudioPlayState, generation: u64) {
    if state.generation == generation {
        state.command = None;
    }
}

/// 通知を反映する。世代違いは捨てる。`Loading` / `Playing` 以外では、一時停止・停止要求の
/// OS 操作の失敗（`Failed`）だけを `Paused` / `Stopped` でも受け入れる（嘘の成功の防止）。
pub fn apply_update(state: &mut AudioPlayState, generation: u64, update: AudioUpdate) {
    if state.generation != generation {
        return;
    }
    let live = matches!(state.phase, AudioPhase::Loading | AudioPhase::Playing);
    let op_failed = matches!(state.phase, AudioPhase::Paused | AudioPhase::Stopped)
        && matches!(update, AudioUpdate::Failed(_));
    if !live && !op_failed {
        return;
    }
    match update {
        AudioUpdate::Snapshot {
            phase,
            position_ms,
            duration_ms,
        } => {
            if matches!(
                phase,
                AudioPhase::Playing | AudioPhase::Ended | AudioPhase::Interrupted
            ) {
                state.phase = phase;
            }
            state.position_ms = position_ms;
            state.duration_ms = duration_ms;
        }
        AudioUpdate::Failed(f) => state.phase = AudioPhase::Failed(f),
    }
}

/// 開始の期限切れ。`Loading` かつ世代一致のときだけ失敗にし、世代を進める（遅れた通知を捨てる）。
pub fn expire(state: &mut AudioPlayState, generation: u64) {
    if state.generation == generation && state.phase == AudioPhase::Loading {
        state.generation = state.generation.wrapping_add(1);
        state.phase = AudioPhase::Failed(AudioFailure::Timeout);
    }
}

/// 案内文言（通常色）。失敗は [`error_text`] 側。
pub fn status_text(state: &AudioPlayState) -> String {
    match state.phase {
        AudioPhase::Idle | AudioPhase::Failed(_) => "",
        AudioPhase::Loading => LOADING_STATUS,
        AudioPhase::Playing => PLAYING_STATUS,
        AudioPhase::Paused => PAUSED_STATUS,
        AudioPhase::Stopped => STOPPED_STATUS,
        AudioPhase::Ended => ENDED_STATUS,
        AudioPhase::Interrupted => INTERRUPTED_STATUS,
    }
    .to_string()
}

/// 失敗の案内（`TextRole::Error`）。失敗以外は空。
pub fn error_text(state: &AudioPlayState) -> String {
    match &state.phase {
        AudioPhase::Failed(AudioFailure::Unsupported) => UNSUPPORTED_STATUS.to_string(),
        AudioPhase::Failed(AudioFailure::Blocked) => BLOCKED_STATUS.to_string(),
        AudioPhase::Failed(AudioFailure::Timeout) => TIMEOUT_STATUS.to_string(),
        AudioPhase::Failed(AudioFailure::Failed(m)) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

fn mmss(ms: Option<u64>) -> String {
    match ms {
        Some(ms) => format!("{}:{:02}", ms / 60_000, ms / 1000 % 60),
        None => UNKNOWN_TIME.to_string(),
    }
}

/// 位置表示（`m:ss / m:ss`、未取得は `--:--`）。未再生で何も取れていなければ空。
pub fn position_text(state: &AudioPlayState) -> String {
    if state.position_ms.is_none() && state.duration_ms.is_none() {
        return String::new();
    }
    format!("{} / {}", mmss(state.position_ms), mmss(state.duration_ms))
}

/// 画面を組み立てる。ボタンは状態によらず 3 つとも出す。
pub fn current_screen(_state: &AudioPlayState) -> Screen<AudioPlayState> {
    Screen::new(TITLE)
        .dynamic_text(status_text)
        .error_text(error_text)
        .dynamic_text(position_text)
        .button(PLAY_BUTTON_ID, PLAY_LABEL, play)
        .button(PAUSE_BUTTON_ID, PAUSE_LABEL, pause)
        .button(STOP_BUTTON_ID, STOP_LABEL, stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> AudioPlayState {
        let mut s = AudioPlayState::default();
        play(&mut s);
        let g = s.generation;
        apply_update(&mut s, g, snap(AudioPhase::Playing, 0));
        s
    }

    fn snap(phase: AudioPhase, pos: u64) -> AudioUpdate {
        AudioUpdate::Snapshot {
            phase,
            position_ms: Some(pos),
            duration_ms: Some(5000),
        }
    }

    #[test]
    fn pause_stop_failure_is_shown() {
        for stop_it in [false, true] {
            let mut s = playing();
            if stop_it {
                stop(&mut s);
            } else {
                pause(&mut s);
            }
            let g = s.generation;
            apply_update(&mut s, g, AudioUpdate::Failed(AudioFailure::Unsupported));
            assert_eq!(s.phase, AudioPhase::Failed(AudioFailure::Unsupported));
        }
    }

    #[test]
    fn transitions_and_commands() {
        let mut s = AudioPlayState::default();
        assert_eq!(take_command(&s), None);
        play(&mut s);
        assert_eq!(take_command(&s), Some((AudioCommand::Play, 1)));
        mark_command_taken(&mut s, 1);
        assert_eq!(take_command(&s), None);
        let g = s.generation;
        apply_update(&mut s, g, snap(AudioPhase::Playing, 100));
        assert_eq!(s.phase, AudioPhase::Playing);
        pause(&mut s);
        assert_eq!(s.phase, AudioPhase::Paused);
        assert_eq!(s.command, Some(AudioCommand::Pause));
        stop(&mut s);
        assert_eq!(s.phase, AudioPhase::Stopped);
        assert_eq!(s.position_ms, Some(0));
        play(&mut s);
        assert_eq!(s.phase, AudioPhase::Loading);
    }

    #[test]
    fn pause_and_stop_are_not_failures() {
        let mut s = playing();
        pause(&mut s);
        assert_eq!(error_text(&s), "");
        stop(&mut s);
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), STOPPED_STATUS);
    }

    #[test]
    fn stale_generation_and_wrong_phase_updates_are_dropped() {
        let mut s = playing();
        let g = s.generation;
        apply_update(
            &mut s,
            g + 1,
            AudioUpdate::Failed(AudioFailure::Unsupported),
        );
        assert_eq!(s.phase, AudioPhase::Playing);
        pause(&mut s);
        let g = s.generation;
        // 一時停止後に遅れて届いた Playing / Ended で状態を戻さない。
        apply_update(&mut s, g, snap(AudioPhase::Ended, 5000));
        assert_eq!(s.phase, AudioPhase::Paused);
    }

    #[test]
    fn expire_only_when_loading_and_same_generation() {
        let mut s = AudioPlayState::default();
        play(&mut s);
        expire(&mut s, 99);
        assert_eq!(s.phase, AudioPhase::Loading);
        let g = s.generation;
        expire(&mut s, g);
        assert_eq!(s.phase, AudioPhase::Failed(AudioFailure::Timeout));
        // 期限切れ後に遅れて届いた Playing は成功にならない。
        apply_update(&mut s, g, snap(AudioPhase::Playing, 0));
        assert_eq!(s.phase, AudioPhase::Failed(AudioFailure::Timeout));
        assert_eq!(error_text(&s), TIMEOUT_STATUS);
    }

    #[test]
    fn ended_is_not_failed_and_interrupted_is_not_failed() {
        let mut s = playing();
        let g = s.generation;
        apply_update(&mut s, g, snap(AudioPhase::Ended, 5000));
        assert_eq!(status_text(&s), ENDED_STATUS);
        assert_eq!(error_text(&s), "");
        play(&mut s);
        let g = s.generation;
        apply_update(&mut s, g, snap(AudioPhase::Interrupted, 1000));
        assert_eq!(status_text(&s), INTERRUPTED_STATUS);
        assert_eq!(error_text(&s), "");
        let mut f = playing();
        let g = f.generation;
        apply_update(
            &mut f,
            g,
            AudioUpdate::Failed(AudioFailure::Failed("デコード".into())),
        );
        assert_eq!(error_text(&f), format!("{FAILED_PREFIX}デコード"));
        assert_eq!(status_text(&f), "");
    }

    #[test]
    fn position_text_uses_placeholder_for_unknown() {
        let mut s = AudioPlayState::default();
        assert_eq!(position_text(&s), "");
        s.position_ms = Some(65_000);
        assert_eq!(position_text(&s), "1:05 / --:--");
        s.duration_ms = Some(5_000);
        assert_eq!(position_text(&s), "1:05 / 0:05");
    }

    #[test]
    fn play_is_ignored_while_loading_or_playing() {
        let mut s = playing();
        let g = s.generation;
        play(&mut s);
        assert_eq!(s.generation, g);
    }
}
