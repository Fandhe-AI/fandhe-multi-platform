//! Phase 11（端末機能の拡充ループ、親 #183）「システムの効果音・ビープ」（Issue #939）。
//! 「ビープ」ボタン → OS 標準の警告音を 1 回要求し、結果（OS に渡した / 鳴らされなかった /
//! 非対応 / 失敗）を表示する 1 画面。`FANDHE_PROXY_APP=system_sound_beep` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/system_sound.rs` が [`take_request`] で要求を観測し
//! [`finish_beep`] で結果を注入する（`haptics_tap` と同型の状態機械）。状態・定数・文言は
//! 全 OS 分をここで定義し、OS 別の実体（macOS / Windows / Android）は画面を触らない。
//!
//! # 「鳴りました」と言い切らない理由
//!
//! `Requested` は OS に渡したまでで、消音・音量・サウンド設定で聞こえないことがある
//! （`fandhe_device::system_sound` の契約）。成功文言は [`REQUESTED_STATUS`] に留める。
//! OS が鳴らさなかったと返した場合（[`BeepResult::NotPlayed`]）は成功にも失敗の色にもしない。
//!
//! ```text
//! Idle --「ビープ」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_sending--> phase=Sending
//!   --finish_beep（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 「ビープ」ボタンの `WidgetId`。
const BEEP_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// OS に渡せたときの案内。聞こえたことは保証しない。
pub const REQUESTED_STATUS: &str =
    "警告音を OS に渡しました（実際に聞こえるかは音量とサウンド設定によります）";
/// OS が鳴らさなかったと返したときの案内（通常色。失敗として扱わない）。
pub const NOT_PLAYED_STATUS: &str = "OS は音を鳴らしませんでした";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str =
    "この OS では使えません（iOS ではアプリから OS 標準の警告音を鳴らせません）";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// ビープ操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BeepPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS へ要求を渡している最中（[`mark_sending`] 済み）。
    Sending,
}

/// OS から返った結果（成功系）。`fandhe_device::system_sound::BeepOutcome` の写し。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeepResult {
    /// OS に渡した。
    Requested,
    /// OS が鳴らさなかったと返した。
    NotPlayed,
}

/// 失敗の種別（画面表示用。`DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeepFailure {
    /// 非対応 OS。
    Unsupported,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeepRequest {
    /// [`finish_beep`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// ビープ画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemSoundBeepState {
    /// 操作の進行状態。
    pub phase: BeepPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。失敗以外では空文字列にする。
    pub error: String,
}

/// 「ビープ」ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn beep(state: &mut SystemSoundBeepState) {
    if state.phase != BeepPhase::Idle {
        return;
    }
    state.phase = BeepPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &SystemSoundBeepState) -> Option<BeepRequest> {
    (state.phase == BeepPhase::Requested).then_some(BeepRequest {
        generation: state.generation,
    })
}

/// 要求を送信中（`Sending`）へ進める。`Requested` 以外では何もしない。
pub fn mark_sending(state: &mut SystemSoundBeepState) {
    if state.phase == BeepPhase::Requested {
        state.phase = BeepPhase::Sending;
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_beep(
    state: &mut SystemSoundBeepState,
    generation: u64,
    result: Result<BeepResult, BeepFailure>,
) {
    if state.phase == BeepPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = BeepPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(BeepResult::Requested) => state.status = REQUESTED_STATUS.to_string(),
        Ok(BeepResult::NotPlayed) => state.status = NOT_PLAYED_STATUS.to_string(),
        Err(BeepFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(BeepFailure::Failed(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &SystemSoundBeepState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &SystemSoundBeepState) -> String {
    state.error.clone()
}

/// ビープ画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &SystemSoundBeepState) -> Screen<SystemSoundBeepState> {
    let screen = Screen::new("システムの効果音")
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != BeepPhase::Idle {
        return screen;
    }
    screen.button(BEEP_BUTTON_ID, "ビープ", beep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn sending() -> SystemSoundBeepState {
        let mut state = SystemSoundBeepState::default();
        beep(&mut state);
        mark_sending(&mut state);
        state
    }

    #[test]
    fn beep_advances_generation_and_phase() {
        let mut state = SystemSoundBeepState::default();
        beep(&mut state);
        assert_eq!(state.phase, BeepPhase::Requested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn beep_is_noop_while_in_flight() {
        let mut state = sending();
        beep(&mut state);
        assert_eq!(state.phase, BeepPhase::Sending);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn take_request_only_when_requested() {
        let mut state = SystemSoundBeepState::default();
        assert_eq!(take_request(&state), None);
        beep(&mut state);
        assert_eq!(take_request(&state), Some(BeepRequest { generation: 1 }));
        mark_sending(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn requested_does_not_claim_it_sounded() {
        let mut state = sending();
        finish_beep(&mut state, 1, Ok(BeepResult::Requested));
        assert_eq!(state.phase, BeepPhase::Idle);
        assert_eq!(state.status, REQUESTED_STATUS);
        assert!(!state.status.contains("鳴りました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn not_played_is_neither_success_nor_error() {
        let mut state = sending();
        finish_beep(&mut state, 1, Ok(BeepResult::NotPlayed));
        assert_eq!(state.status, NOT_PLAYED_STATUS);
        assert_ne!(state.status, REQUESTED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn unsupported_and_failed_are_errors_without_success_text() {
        let mut state = sending();
        finish_beep(&mut state, 1, Err(BeepFailure::Unsupported));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.status, "");

        let mut state = sending();
        finish_beep(&mut state, 1, Err(BeepFailure::Failed("x".to_string())));
        assert_eq!(state.error, format!("{FAILED_PREFIX}x"));
        assert_eq!(state.status, "");
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = sending();
        finish_beep(&mut state, 99, Ok(BeepResult::Requested));
        assert_eq!(state.phase, BeepPhase::Sending);
        let mut idle = SystemSoundBeepState::default();
        finish_beep(&mut idle, 0, Ok(BeepResult::Requested));
        assert_eq!(idle.status, "");
    }

    #[test]
    fn button_is_hidden_while_in_flight_and_shown_when_idle() {
        let mut state = SystemSoundBeepState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "ビープ", &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "ビープ", &mut state));
    }
}
