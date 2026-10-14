//! Phase 11（端末機能の拡充ループ、親 #1090）型サンプル「触覚フィードバック」（Issue #1091）。
//! 「振動」ボタン → OS へ軽い振動 1 回を要求し、結果（要求した/非搭載/失敗）を表示する 1 画面。
//! `FANDHE_PROXY_APP=haptics_tap` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod haptics_tap;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。OS 呼び出しは本モジュール外
//! （`fandhe-egui-proxy` の `device/haptics.rs`、Web は `fandhe-web-dom` の `haptics.rs`）が
//! [`take_request`] で要求を観測し [`finish_tap`] で結果を注入する
//! （`notification_send.rs` と同型の状態機械）。
//!
//! # 「振動しました」と言い切らない理由
//!
//! `Ok` の意味は OS ごとに異なり、macOS・Web は実際に振動したかを検知できない
//! （`fandhe_device::haptics` の契約節）。そのため成功文言は [`REQUESTED_STATUS`] に留める
//! （`device-bridge-contracts.md`「6. 嘘の成功」）。
//!
//! # 連打
//!
//! 振動の完了は待たない。ボタンは要求を OS へ渡している間（`Requested`/`Sending`）だけ外れ、
//! 完了通知の直後から次の押下を受け付ける。
//!
//! ```text
//! Idle --「振動」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_sending--> phase=Sending
//!   --finish_tap（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 「振動」ボタンの `WidgetId`。
const TAP_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// OS へ要求を渡せたときの案内。振動したことは保証しない。
pub const REQUESTED_STATUS: &str = "振動を要求しました（実際に振動するかは端末と設定によります）";
/// 非搭載・API 無しのときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この端末では使えません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 振動操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HapticsPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS へ要求を渡している最中（[`mark_sending`] 済み）。
    Sending,
}

/// 失敗の種別（画面表示用。`fandhe_device::DeviceError` から写す責務は effect 側、
/// `fandhe-core` は `fandhe-device` に依存しないため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapFailure {
    /// 非搭載・API 無し。
    Unsupported,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HapticsRequest {
    /// [`finish_tap`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 振動画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HapticsTapState {
    /// 操作の進行状態。
    pub phase: HapticsPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 成功の案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列にする。
    pub error: String,
}

/// 「振動」ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn tap(state: &mut HapticsTapState) {
    if state.phase != HapticsPhase::Idle {
        return;
    }
    state.phase = HapticsPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果表示を残さない（`notification_send::notify` と同じ判断）。
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &HapticsTapState) -> Option<HapticsRequest> {
    (state.phase == HapticsPhase::Requested).then_some(HapticsRequest {
        generation: state.generation,
    })
}

/// 要求を送信中（`Sending`）へ進める。`Requested` 以外では何もしない。
pub fn mark_sending(state: &mut HapticsTapState) {
    if state.phase == HapticsPhase::Requested {
        state.phase = HapticsPhase::Sending;
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_tap(state: &mut HapticsTapState, generation: u64, result: Result<(), TapFailure>) {
    if state.phase == HapticsPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = HapticsPhase::Idle;
    match result {
        Ok(()) => {
            state.status = REQUESTED_STATUS.to_string();
            state.error = String::new();
        }
        Err(TapFailure::Unsupported) => {
            state.status = String::new();
            state.error = UNSUPPORTED_STATUS.to_string();
        }
        Err(TapFailure::Failed(message)) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &HapticsTapState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &HapticsTapState) -> String {
    state.error.clone()
}

/// 振動画面を組み立てる。`registry.rs` の `effect` 区画から `fn(&S) -> Screen<S>` として
/// 呼ばれる契約。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &HapticsTapState) -> Screen<HapticsTapState> {
    let screen = Screen::new("触覚フィードバック")
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != HapticsPhase::Idle {
        return screen;
    }
    screen.button(TAP_BUTTON_ID, "振動", tap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn sending() -> HapticsTapState {
        let mut state = HapticsTapState::default();
        tap(&mut state);
        mark_sending(&mut state);
        state
    }

    #[test]
    fn tap_advances_generation_and_phase() {
        let mut state = HapticsTapState::default();
        tap(&mut state);
        assert_eq!(state.phase, HapticsPhase::Requested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn tap_is_noop_while_in_flight() {
        let mut state = sending();
        tap(&mut state);
        assert_eq!(state.phase, HapticsPhase::Sending);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn take_request_only_when_requested() {
        let mut state = HapticsTapState::default();
        assert_eq!(take_request(&state), None);
        tap(&mut state);
        assert_eq!(take_request(&state), Some(HapticsRequest { generation: 1 }));
        mark_sending(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn finish_ok_says_requested_not_vibrated() {
        let mut state = sending();
        finish_tap(&mut state, 1, Ok(()));
        assert_eq!(state.phase, HapticsPhase::Idle);
        assert_eq!(state.status, REQUESTED_STATUS);
        assert!(!state.status.contains("振動しました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_unsupported_is_error_not_success() {
        let mut state = sending();
        finish_tap(&mut state, 1, Err(TapFailure::Unsupported));
        assert_eq!(state.phase, HapticsPhase::Idle);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert!(!state.status.contains("要求しました"));
    }

    #[test]
    fn finish_failed_has_prefix_and_no_success_text() {
        let mut state = sending();
        finish_tap(&mut state, 1, Err(TapFailure::Failed("x".to_string())));
        assert_eq!(state.error, format!("{FAILED_PREFIX}x"));
        assert_eq!(state.status, "");
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = sending();
        finish_tap(&mut state, 99, Ok(()));
        assert_eq!(state.phase, HapticsPhase::Sending);
        let mut idle = HapticsTapState::default();
        finish_tap(&mut idle, 0, Ok(()));
        assert_eq!(idle.status, "");
    }

    #[test]
    fn button_is_hidden_while_in_flight_and_shown_when_idle() {
        let mut state = HapticsTapState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "振動", &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "振動", &mut state));
    }
}
