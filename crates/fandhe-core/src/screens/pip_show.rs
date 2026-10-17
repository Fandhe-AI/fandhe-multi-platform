//! Phase 11（端末機能の拡充ループ、親 #97）型サンプル「ピクチャ・イン・ピクチャ」（Issue #364）。
//! 「小窓にする」「小窓をやめる」の 2 ボタンで、実行時に生成した単色映像（5 色を 1 秒ごとに切り替え）を
//! OS の小窓へ出す 1 画面。`FANDHE_PROXY_APP=pip_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod pip_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/picture_in_picture.rs` が [`take_command`] で要求を観測し、
//! [`apply_update`] で通知を注入する（`audio_play.rs` と同型の状態機械）。
//!
//! 全対象 OS（macOS / iOS / Android / Web）の状態・文言をここで確定済みで、後続の OS 実装は本画面を
//! 触らない。ボタンは Web の `mount` が `Screen` を作り直さない制約のため状態によらず常に出す。
//!
//! ```text
//! 画面を開く: Idle --prepare--> Preparing --Phase(Ready)--> Ready
//! 「小窓にする」: Ready/Exited --> Starting --Phase(Active)--> Active
//! 「小窓をやめる」: Starting/Active --> Stopping --Phase(Exited)--> Exited
//! Active --OS の小窓側の閉じる・戻す--> Exited（失敗ではない）
//! Preparing/Starting/Stopping --期限切れ expire--> Failed(Timeout)
//! ```
//!
//! 画面内プレビュー（`preview_rgb`）は小窓に出す映像と同じ色表で、macOS・iOS ではネイティブの映像と並ぶ
//! 目印、Android では小窓に入る「元の映像」そのもの。世代の違う通知と、遷移中・小窓中以外で届いた
//! 通知は捨てる。

use crate::{RgbaPixels, Screen, WidgetId};

const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 画面タイトル。
pub const TITLE: &str = "ピクチャ・イン・ピクチャ";
/// 「小窓にする」ボタンのラベル。
pub const START_LABEL: &str = "小窓にする";
/// 「小窓をやめる」ボタンのラベル。
pub const STOP_LABEL: &str = "小窓をやめる";
/// 準備中の案内。
pub const PREPARING_STATUS: &str = "準備しています…";
/// 小窓にできるときの案内。
pub const READY_STATUS: &str = "小窓にできます";
/// 小窓へ切り替え中の案内。
pub const STARTING_STATUS: &str = "小窓に切り替えています…";
/// 小窓中の案内。
pub const ACTIVE_STATUS: &str = "小窓に出しています（小窓の操作でも戻せます）";
/// 小窓から戻している最中の案内。
pub const STOPPING_STATUS: &str = "小窓から戻しています…";
/// 小窓から戻ったときの案内（失敗ではない）。
pub const EXITED_STATUS: &str = "小窓から戻りました";
/// 非対応のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では小窓にできません";
/// OS が小窓にすることを許さないときの案内。
pub const NOT_POSSIBLE_STATUS: &str = "いまは小窓にできません（端末の設定を確認してください）";
/// ブラウザに止められたときの案内（Web）。
pub const NOT_ALLOWED_STATUS: &str = "ブラウザに止められました。もう一度押してください";
/// 期限切れの案内。
pub const TIMEOUT_STATUS: &str = "小窓に切り替えられませんでした（期限切れ）";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 画面内プレビューの代替テキスト。
pub const PREVIEW_ALT: &str = "小窓に出す映像（1 秒ごとに色が変わります）";

/// 画面内プレビューの寸法（16:9）。
const PREVIEW_W: u32 = 16;
const PREVIEW_H: u32 = 9;

/// 画面側の進行状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PipShowPhase {
    /// 未準備。
    #[default]
    Idle,
    /// 準備中（可否の確定待ち）。
    Preparing,
    /// 小窓にできる。
    Ready,
    /// 小窓への切り替え中。
    Starting,
    /// 小窓に出ている。
    Active,
    /// 小窓から戻している最中。
    Stopping,
    /// 小窓から戻った。失敗ではない。
    Exited,
    /// 失敗。
    Failed(PipShowFailure),
}

/// 失敗の種別（`fandhe_device` の型から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipShowFailure {
    /// 非対応。
    Unsupported,
    /// OS がいまは小窓にすることを許さない。
    NotPossible,
    /// ユーザー操作の外・拒否で止められた（Web）。
    NotAllowed,
    /// 期限切れ。
    Timeout,
    /// それ以外（固定文言のみ。OS の生コードを入れない）。
    Failed(String),
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipCommand {
    /// 映像源と controller を準備する。
    Prepare,
    /// 小窓にする。
    Start,
    /// 小窓をやめる。
    Stop,
}

/// effect ドライバから注入する通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipShowUpdate {
    /// 状態。`Ready` / `Active` / `Exited` のみ意味を持つ。
    Phase(PipShowPhase),
    /// 失敗。
    Failed(PipShowFailure),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PipShowState {
    /// 進行状態。
    pub phase: PipShowPhase,
    /// 直近に立てた要求の世代。
    pub generation: u64,
    /// 未発送の要求。
    pub command: Option<PipCommand>,
    /// 画面内プレビューの色（未準備・失敗では `None`）。
    pub preview_rgb: Option<[u8; 3]>,
}

fn issue(state: &mut PipShowState, command: PipCommand) {
    state.generation = state.generation.wrapping_add(1);
    state.command = Some(command);
}

/// 準備を始める。`Idle` または `Unsupported` 以外の `Failed` のときだけ受け付ける
/// （画面を開いたときの自動準備と、失敗からのやり直しで使う）。
pub fn prepare(state: &mut PipShowState) {
    match state.phase {
        PipShowPhase::Idle => {}
        PipShowPhase::Failed(PipShowFailure::Unsupported) => return,
        PipShowPhase::Failed(_) => {}
        _ => return,
    }
    issue(state, PipCommand::Prepare);
    state.phase = PipShowPhase::Preparing;
    state.preview_rgb = None;
}

/// 「小窓にする」。`Ready` / `Exited` のときだけ小窓へ進む。`Idle` と `Unsupported` 以外の `Failed` は
/// 準備からやり直す。遷移中・小窓中・`Unsupported` では何もしない。
pub fn start(state: &mut PipShowState) {
    match state.phase {
        PipShowPhase::Ready | PipShowPhase::Exited => {
            issue(state, PipCommand::Start);
            state.phase = PipShowPhase::Starting;
        }
        PipShowPhase::Idle | PipShowPhase::Failed(_) => prepare(state),
        _ => {}
    }
}

/// 「小窓をやめる」。`Starting` / `Active` のときだけ受け付ける。失敗ではなく `Exited` へ向かう。
pub fn stop(state: &mut PipShowState) {
    if !matches!(state.phase, PipShowPhase::Starting | PipShowPhase::Active) {
        return;
    }
    issue(state, PipCommand::Stop);
    state.phase = PipShowPhase::Stopping;
}

/// 未発送の要求と世代を観測する getter（状態は変えない）。
pub fn take_command(state: &PipShowState) -> Option<(PipCommand, u64)> {
    state.command.map(|c| (c, state.generation))
}

/// 要求を発送済みにする。世代が違えば何もしない。
pub fn mark_command_taken(state: &mut PipShowState, generation: u64) {
    if state.generation == generation {
        state.command = None;
    }
}

/// 通知を反映する。世代違いは捨てる。現在の状態から意味のある遷移だけを受ける
/// （`Preparing` → `Ready`、`Starting` → `Active` / `Exited`、`Active` / `Stopping` → `Exited`）。
/// 失敗は遷移中・小窓中のとき受ける。
pub fn apply_update(state: &mut PipShowState, generation: u64, update: PipShowUpdate) {
    if state.generation != generation {
        return;
    }
    use PipShowPhase as P;
    match update {
        PipShowUpdate::Phase(next) => {
            let ok = matches!(
                (&state.phase, &next),
                (P::Preparing, P::Ready)
                    | (P::Starting, P::Active | P::Exited)
                    | (P::Active | P::Stopping, P::Exited)
            );
            if ok {
                state.phase = next;
            }
        }
        PipShowUpdate::Failed(f) => {
            if matches!(
                state.phase,
                P::Preparing | P::Starting | P::Active | P::Stopping
            ) {
                state.phase = P::Failed(f);
                state.preview_rgb = None;
            }
        }
    }
}

/// 画面内プレビューの色を更新する。`Idle` / `Failed` では捨てる。
pub fn set_preview(state: &mut PipShowState, rgb: [u8; 3]) {
    if matches!(state.phase, PipShowPhase::Idle | PipShowPhase::Failed(_)) {
        return;
    }
    state.preview_rgb = Some(rgb);
}

/// 遷移の期限切れ。`Preparing` / `Starting` / `Stopping` かつ世代一致のときだけ失敗にし、
/// 世代を進める（遅れて届く通知を捨てる）。
pub fn expire(state: &mut PipShowState, generation: u64) {
    if state.generation == generation
        && matches!(
            state.phase,
            PipShowPhase::Preparing | PipShowPhase::Starting | PipShowPhase::Stopping
        )
    {
        state.generation = state.generation.wrapping_add(1);
        state.phase = PipShowPhase::Failed(PipShowFailure::Timeout);
        state.preview_rgb = None;
    }
}

/// 案内文言（通常色）。失敗は [`error_text`] 側。`Exited` と利用者の停止はここに出す。
pub fn status_text(state: &PipShowState) -> String {
    match state.phase {
        PipShowPhase::Idle | PipShowPhase::Failed(_) => "",
        PipShowPhase::Preparing => PREPARING_STATUS,
        PipShowPhase::Ready => READY_STATUS,
        PipShowPhase::Starting => STARTING_STATUS,
        PipShowPhase::Active => ACTIVE_STATUS,
        PipShowPhase::Stopping => STOPPING_STATUS,
        PipShowPhase::Exited => EXITED_STATUS,
    }
    .to_string()
}

/// 失敗の案内（`TextRole::Error`）。失敗以外は空。
pub fn error_text(state: &PipShowState) -> String {
    match &state.phase {
        PipShowPhase::Failed(PipShowFailure::Unsupported) => UNSUPPORTED_STATUS.to_string(),
        PipShowPhase::Failed(PipShowFailure::NotPossible) => NOT_POSSIBLE_STATUS.to_string(),
        PipShowPhase::Failed(PipShowFailure::NotAllowed) => NOT_ALLOWED_STATUS.to_string(),
        PipShowPhase::Failed(PipShowFailure::Timeout) => TIMEOUT_STATUS.to_string(),
        PipShowPhase::Failed(PipShowFailure::Failed(m)) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

fn preview_pixels(rgb: [u8; 3]) -> Option<RgbaPixels> {
    let rgba: Vec<u8> = (0..PREVIEW_W * PREVIEW_H)
        .flat_map(|_| [rgb[0], rgb[1], rgb[2], 0xFF])
        .collect();
    RgbaPixels::new(PREVIEW_W, PREVIEW_H, rgba)
}

/// 画面を組み立てる。ボタンは状態によらず 2 つとも出す。
pub fn current_screen(state: &PipShowState) -> Screen<PipShowState> {
    let mut screen = Screen::new(TITLE);
    if let Some(pixels) = state.preview_rgb.and_then(preview_pixels) {
        screen = screen.rgba_image(pixels, PREVIEW_ALT);
    }
    screen
        .dynamic_text(status_text)
        .error_text(error_text)
        .button(START_BUTTON_ID, START_LABEL, start)
        .button(STOP_BUTTON_ID, STOP_LABEL, stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(phase: PipShowPhase) -> PipShowState {
        PipShowState {
            phase,
            generation: 3,
            ..Default::default()
        }
    }

    fn phase_update(s: &mut PipShowState, p: PipShowPhase) {
        let g = s.generation;
        apply_update(s, g, PipShowUpdate::Phase(p));
    }

    #[test]
    fn happy_path_and_commands() {
        let mut s = PipShowState::default();
        prepare(&mut s);
        assert_eq!(take_command(&s), Some((PipCommand::Prepare, 1)));
        mark_command_taken(&mut s, 1);
        phase_update(&mut s, PipShowPhase::Ready);
        start(&mut s);
        assert_eq!(s.phase, PipShowPhase::Starting);
        assert_eq!(s.command, Some(PipCommand::Start));
        phase_update(&mut s, PipShowPhase::Active);
        assert_eq!(s.phase, PipShowPhase::Active);
        stop(&mut s);
        assert_eq!(s.phase, PipShowPhase::Stopping);
        assert_eq!(s.command, Some(PipCommand::Stop));
        phase_update(&mut s, PipShowPhase::Exited);
        assert_eq!(s.phase, PipShowPhase::Exited);
        // 戻った後はもう一度小窓にできる
        start(&mut s);
        assert_eq!(s.phase, PipShowPhase::Starting);
    }

    #[test]
    fn start_is_ignored_while_busy() {
        for p in [
            PipShowPhase::Preparing,
            PipShowPhase::Starting,
            PipShowPhase::Active,
            PipShowPhase::Stopping,
            PipShowPhase::Failed(PipShowFailure::Unsupported),
        ] {
            let mut s = at(p.clone());
            start(&mut s);
            assert_eq!(s.phase, p);
            assert_eq!(s.command, None);
            assert_eq!(s.generation, 3);
        }
    }

    #[test]
    fn start_after_failure_retries_prepare() {
        let mut s = at(PipShowPhase::Failed(PipShowFailure::NotPossible));
        start(&mut s);
        assert_eq!(s.phase, PipShowPhase::Preparing);
        assert_eq!(s.command, Some(PipCommand::Prepare));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = at(PipShowPhase::Starting);
        apply_update(&mut s, 2, PipShowUpdate::Phase(PipShowPhase::Active));
        apply_update(&mut s, 2, PipShowUpdate::Failed(PipShowFailure::NotAllowed));
        assert_eq!(s.phase, PipShowPhase::Starting);
    }

    #[test]
    fn expire_only_while_transitioning() {
        for p in [
            PipShowPhase::Ready,
            PipShowPhase::Active,
            PipShowPhase::Exited,
        ] {
            let mut s = at(p.clone());
            expire(&mut s, 3);
            assert_eq!(s.phase, p);
        }
        let mut s = at(PipShowPhase::Starting);
        expire(&mut s, 3);
        assert_eq!(s.phase, PipShowPhase::Failed(PipShowFailure::Timeout));
        assert_eq!(s.generation, 4);
    }

    #[test]
    fn active_after_expire_is_dropped() {
        let mut s = at(PipShowPhase::Starting);
        expire(&mut s, 3);
        apply_update(&mut s, 3, PipShowUpdate::Phase(PipShowPhase::Active));
        assert_eq!(s.phase, PipShowPhase::Failed(PipShowFailure::Timeout));
    }

    #[test]
    fn exited_is_status_not_error() {
        let s = at(PipShowPhase::Exited);
        assert_eq!(status_text(&s), EXITED_STATUS);
        assert_eq!(error_text(&s), "");
        let f = at(PipShowPhase::Failed(PipShowFailure::Failed("x".into())));
        assert_eq!(error_text(&f), format!("{FAILED_PREFIX}x"));
        assert_eq!(status_text(&f), "");
    }

    #[test]
    fn preview_is_dropped_when_idle_or_failed() {
        for p in [
            PipShowPhase::Idle,
            PipShowPhase::Failed(PipShowFailure::Timeout),
        ] {
            let mut s = at(p);
            set_preview(&mut s, [1, 2, 3]);
            assert_eq!(s.preview_rgb, None);
        }
        let mut s = at(PipShowPhase::Ready);
        set_preview(&mut s, [1, 2, 3]);
        assert_eq!(s.preview_rgb, Some([1, 2, 3]));
        assert!(preview_pixels([1, 2, 3]).is_some());
    }

    #[test]
    fn stop_only_while_starting_or_active() {
        let mut s = at(PipShowPhase::Ready);
        stop(&mut s);
        assert_eq!(s.phase, PipShowPhase::Ready);
        let mut s = at(PipShowPhase::Starting);
        stop(&mut s);
        assert_eq!(s.phase, PipShowPhase::Stopping);
    }
}
