//! Phase 11（端末機能の拡充ループ、親 #174）型サンプル「Dock・タスクバーの進捗表示と注意喚起」
//! （Issue #860）。進捗（0〜100 %）の入力欄と「設定する」「消す」、注意喚起の「背面に回ったら
//! 注意を引く」「取り消す」だけの 1 画面。`FANDHE_PROXY_APP=app_progress_attention_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod app_progress_attention_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md` の制約どおり）。
//! 範囲検証と OS 呼び出しは effect ドライバ（`device/app_progress_attention.rs`）が
//! [`take_progress_request`] / [`take_attention_request`] で要求を観測して行い、
//! [`finish_progress`] / [`finish_attention`] で結果を注入する。全 OS 分の文言は (a) で確定する。
//!
//! # 進捗と注意喚起は独立
//!
//! 結果欄・世代・進行状態を別々に持ち、一方の結果が他方を上書きしない。注意喚起は
//! 「待機（Armed）」に入り、ドライバがウィンドウのフォーカス喪失を観測した最初のフレームで
//! 1 回だけ OS を呼ぶ（前面のままの呼び出しは OS が無視するため、ボタンで即時には呼ばない）。
//! 待機の取り消しは OS を呼ばず、失敗にもしない。世代番号で、古い完了や `Idle` で届いた完了を
//! 無視する。

use crate::{Screen, WidgetId};

/// 「設定する」ボタンの `WidgetId`。
const SET_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「消す」ボタンの `WidgetId`。
const CLEAR_BUTTON_ID: WidgetId = WidgetId::from_raw(3);
/// 「背面に回ったら注意を引く」ボタンの `WidgetId`。
const ARM_BUTTON_ID: WidgetId = WidgetId::from_raw(4);
/// 「取り消す」ボタンの `WidgetId`。
const CANCEL_BUTTON_ID: WidgetId = WidgetId::from_raw(5);

/// 進捗を OS へ渡せたときの案内（表示は確かめられない）。
pub const PROGRESS_SUBMITTED_STATUS: &str = "進捗を OS へ渡しました（表示は確かめられません）";
/// 進捗の消去を OS へ依頼できたときの案内。
pub const PROGRESS_CLEARED_STATUS: &str = "進捗の消去を OS へ依頼しました";
/// 注意喚起の待機に入ったときの案内。
pub const ATTENTION_ARMED_STATUS: &str = "待機中です。ウィンドウが背面に回ると注意を引きます";
/// 注意喚起を OS へ渡せたときの案内。
pub const ATTENTION_SUBMITTED_STATUS: &str =
    "注意喚起を OS へ渡しました（アプリへ戻ると止まります）";
/// 待機を取り消したときの案内。OS は呼んでいない。
pub const ATTENTION_CANCELLED_STATUS: &str = "待機を取り消しました（OS は呼んでいません）";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境ではサポートされていません";
/// 数値でない入力の案内。
pub const INVALID_NUMBER_STATUS: &str = "0 以上の整数を入力してください";
/// 範囲外の失敗文言。
pub const OUT_OF_RANGE_MESSAGE: &str = "進捗は 0〜100 で入力してください";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "実行できませんでした: ";
/// 固定の注記。
pub const NOTE_TEXT: &str = "macOS は Dock のアイコンに進捗バーを描きます。Windows はタスクバーボタンの進捗で、\
ハイコントラストでは出ません。注意喚起は前面のままでは何も起きないため、背面に回るまで待ちます。\
macOS / Windows 以外は非対応です";

/// 進捗操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_progress_setting`] 済み）。
    Setting,
}

/// 注意喚起の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AttentionPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// 背面に回るのを待っている（OS は未呼び出し）。
    Armed,
    /// effect ドライバが OS を呼んでいる（[`mark_attention_requesting`] 済み）。
    Requesting,
}

/// 進捗の操作内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressOp {
    /// 値（%）を設定する。範囲はドライバが検証する。
    Set(u32),
    /// 消す。
    #[default]
    Clear,
}

/// 進捗の OS 結果（写像は `fandhe-egui-proxy` 側の責務）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressOutcome {
    /// 進捗を OS へ渡した。
    Submitted,
    /// 消去を OS へ依頼した。
    Cleared,
    /// この環境では非対応。
    Unsupported,
}

/// 注意喚起の OS 結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionOutcome {
    /// 注意喚起を OS へ渡した。
    Submitted,
    /// この環境では非対応。
    Unsupported,
}

/// effect ドライバへ渡す進捗の要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressRequest {
    /// [`finish_progress`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 操作内容。
    pub op: ProgressOp,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppProgressAttentionState {
    /// 入力欄の文字列。
    pub input: String,
    /// 進捗の進行状態。
    pub progress_phase: ProgressPhase,
    /// 進捗の直近の操作。
    pub progress_op: ProgressOp,
    /// 進捗の直近の要求の世代番号。
    pub progress_generation: u64,
    /// 進捗の案内（通常色）。
    pub progress_status: String,
    /// 進捗の失敗（`TextRole::Error`）。
    pub progress_error: String,
    /// 注意喚起の進行状態。
    pub attention_phase: AttentionPhase,
    /// 注意喚起の直近の要求の世代番号。
    pub attention_generation: u64,
    /// 注意喚起の案内（通常色）。
    pub attention_status: String,
    /// 注意喚起の失敗（`TextRole::Error`）。
    pub attention_error: String,
}

fn request_progress(state: &mut AppProgressAttentionState, op: ProgressOp) {
    state.progress_phase = ProgressPhase::Requested;
    state.progress_generation = state.progress_generation.wrapping_add(1);
    state.progress_op = op;
    state.progress_status = String::new();
    state.progress_error = String::new();
}

/// 「設定する」ボタンのハンドラ。`Idle` 以外は no-op。数値でなければ要求を立てない。
pub fn set_progress(state: &mut AppProgressAttentionState) {
    if state.progress_phase != ProgressPhase::Idle {
        return;
    }
    match state.input.trim().parse::<u32>() {
        Ok(v) => request_progress(state, ProgressOp::Set(v)),
        Err(_) => {
            state.progress_status = String::new();
            state.progress_error = INVALID_NUMBER_STATUS.to_string();
        }
    }
}

/// 「消す」ボタンのハンドラ。`Idle` 以外は no-op。
pub fn clear_progress(state: &mut AppProgressAttentionState) {
    if state.progress_phase != ProgressPhase::Idle {
        return;
    }
    request_progress(state, ProgressOp::Clear);
}

/// 「背面に回ったら注意を引く」ボタンのハンドラ。`Idle` 以外は no-op。
pub fn arm_attention(state: &mut AppProgressAttentionState) {
    if state.attention_phase != AttentionPhase::Idle {
        return;
    }
    state.attention_phase = AttentionPhase::Armed;
    state.attention_generation = state.attention_generation.wrapping_add(1);
    state.attention_status = ATTENTION_ARMED_STATUS.to_string();
    state.attention_error = String::new();
}

/// 「取り消す」ボタンのハンドラ。待機（`Armed`）のときだけ効く。OS は呼ばず、失敗にもしない。
pub fn cancel_attention(state: &mut AppProgressAttentionState) {
    if state.attention_phase != AttentionPhase::Armed {
        return;
    }
    state.attention_phase = AttentionPhase::Idle;
    state.attention_generation = state.attention_generation.wrapping_add(1);
    state.attention_status = ATTENTION_CANCELLED_STATUS.to_string();
    state.attention_error = String::new();
}

/// effect ドライバが未発送の進捗要求を観測する（状態は変えない）。
pub fn take_progress_request(state: &AppProgressAttentionState) -> Option<ProgressRequest> {
    (state.progress_phase == ProgressPhase::Requested).then_some(ProgressRequest {
        generation: state.progress_generation,
        op: state.progress_op,
    })
}

/// 進捗要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_progress_setting(state: &mut AppProgressAttentionState) {
    if state.progress_phase == ProgressPhase::Requested {
        state.progress_phase = ProgressPhase::Setting;
    }
}

/// 進捗の結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_progress(
    state: &mut AppProgressAttentionState,
    generation: u64,
    result: Result<ProgressOutcome, String>,
) {
    if state.progress_phase == ProgressPhase::Idle || state.progress_generation != generation {
        return;
    }
    state.progress_phase = ProgressPhase::Idle;
    state.progress_status = String::new();
    state.progress_error = String::new();
    match result {
        Ok(ProgressOutcome::Submitted) => {
            state.progress_status = PROGRESS_SUBMITTED_STATUS.to_string()
        }
        Ok(ProgressOutcome::Cleared) => state.progress_status = PROGRESS_CLEARED_STATUS.to_string(),
        Ok(ProgressOutcome::Unsupported) => state.progress_error = UNSUPPORTED_STATUS.to_string(),
        Err(m) => state.progress_error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// effect ドライバが注意喚起を発火すべきか観測する。待機中かつウィンドウがフォーカスを
/// 持っていないときだけ世代を返す（状態は変えない）。フォーカスは OS の観測値（egui の
/// `raw.focused`）で、アプリ内に覚えない。
pub fn take_attention_request(state: &AppProgressAttentionState, focused: bool) -> Option<u64> {
    (state.attention_phase == AttentionPhase::Armed && !focused)
        .then_some(state.attention_generation)
}

/// 注意喚起を処理中へ進める。`Armed` 以外では何もしない。
pub fn mark_attention_requesting(state: &mut AppProgressAttentionState) {
    if state.attention_phase == AttentionPhase::Armed {
        state.attention_phase = AttentionPhase::Requesting;
    }
}

/// 注意喚起の結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_attention(
    state: &mut AppProgressAttentionState,
    generation: u64,
    result: Result<AttentionOutcome, String>,
) {
    if state.attention_phase != AttentionPhase::Requesting
        || state.attention_generation != generation
    {
        return;
    }
    state.attention_phase = AttentionPhase::Idle;
    state.attention_status = String::new();
    state.attention_error = String::new();
    match result {
        Ok(AttentionOutcome::Submitted) => {
            state.attention_status = ATTENTION_SUBMITTED_STATUS.to_string()
        }
        Ok(AttentionOutcome::Unsupported) => state.attention_error = UNSUPPORTED_STATUS.to_string(),
        Err(m) => state.attention_error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 進捗の案内文言。
pub fn progress_status_text(state: &AppProgressAttentionState) -> String {
    state.progress_status.clone()
}

/// 進捗のエラー文言。
pub fn progress_error_text(state: &AppProgressAttentionState) -> String {
    state.progress_error.clone()
}

/// 注意喚起の案内文言。
pub fn attention_status_text(state: &AppProgressAttentionState) -> String {
    state.attention_status.clone()
}

/// 注意喚起のエラー文言。
pub fn attention_error_text(state: &AppProgressAttentionState) -> String {
    state.attention_error.clone()
}

fn input_get(state: &AppProgressAttentionState) -> String {
    state.input.clone()
}

fn input_set(state: &mut AppProgressAttentionState, v: String) {
    state.input = v;
}

/// 画面を組み立てる。処理中・待機中は該当ボタンを外して二重押しを防ぐ。
pub fn current_screen(state: &AppProgressAttentionState) -> Screen<AppProgressAttentionState> {
    let mut screen = Screen::new("進捗表示と注意喚起")
        .text(NOTE_TEXT)
        .text_field("進捗（0〜100 %）", input_get, input_set)
        .dynamic_text(progress_status_text)
        .error_text(progress_error_text)
        .dynamic_text(attention_status_text)
        .error_text(attention_error_text);
    if state.progress_phase == ProgressPhase::Idle {
        screen = screen
            .button(SET_BUTTON_ID, "設定する", set_progress)
            .button(CLEAR_BUTTON_ID, "消す", clear_progress);
    }
    match state.attention_phase {
        AttentionPhase::Idle => {
            screen.button(ARM_BUTTON_ID, "背面に回ったら注意を引く", arm_attention)
        }
        AttentionPhase::Armed => screen.button(CANCEL_BUTTON_ID, "取り消す", cancel_attention),
        AttentionPhase::Requesting => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(input: &str) -> AppProgressAttentionState {
        let mut s = AppProgressAttentionState {
            input: input.into(),
            ..Default::default()
        };
        set_progress(&mut s);
        mark_progress_setting(&mut s);
        s
    }

    fn requesting() -> AppProgressAttentionState {
        let mut s = AppProgressAttentionState::default();
        arm_attention(&mut s);
        mark_attention_requesting(&mut s);
        s
    }

    #[test]
    fn progress_request_advances_generation_and_phase() {
        let mut s = AppProgressAttentionState {
            input: " 42 ".into(),
            ..Default::default()
        };
        set_progress(&mut s);
        let r = take_progress_request(&s).unwrap();
        assert_eq!((r.generation, r.op), (1, ProgressOp::Set(42)));
        mark_progress_setting(&mut s);
        assert!(take_progress_request(&s).is_none());
    }

    #[test]
    fn non_numeric_input_does_not_request() {
        for bad in ["", "abc", "-1", "1.5"] {
            let mut s = AppProgressAttentionState {
                input: bad.into(),
                ..Default::default()
            };
            set_progress(&mut s);
            assert_eq!(s.progress_phase, ProgressPhase::Idle, "{bad}");
            assert_eq!(s.progress_error, INVALID_NUMBER_STATUS);
            assert!(take_progress_request(&s).is_none());
        }
    }

    #[test]
    fn handlers_are_noop_while_in_flight() {
        let mut s = setting("5");
        let before = s.clone();
        set_progress(&mut s);
        clear_progress(&mut s);
        assert_eq!(s, before);

        let mut a = requesting();
        let before = a.clone();
        arm_attention(&mut a);
        cancel_attention(&mut a);
        assert_eq!(a, before);
    }

    #[test]
    fn stale_and_idle_progress_completions_are_ignored() {
        let mut s = setting("3");
        let g = s.progress_generation + 1;
        finish_progress(&mut s, g, Ok(ProgressOutcome::Submitted));
        assert_eq!(s.progress_phase, ProgressPhase::Setting);
        assert!(s.progress_status.is_empty());

        let mut idle = AppProgressAttentionState::default();
        finish_progress(&mut idle, 0, Ok(ProgressOutcome::Submitted));
        assert!(idle.progress_status.is_empty());
    }

    #[test]
    fn stale_and_idle_attention_completions_are_ignored() {
        let mut s = requesting();
        let g = s.attention_generation + 1;
        finish_attention(&mut s, g, Ok(AttentionOutcome::Submitted));
        assert_eq!(s.attention_phase, AttentionPhase::Requesting);

        let mut armed = AppProgressAttentionState::default();
        arm_attention(&mut armed);
        finish_attention(&mut armed, 1, Ok(AttentionOutcome::Submitted));
        assert_eq!(armed.attention_phase, AttentionPhase::Armed);

        let mut idle = AppProgressAttentionState::default();
        finish_attention(&mut idle, 0, Ok(AttentionOutcome::Submitted));
        assert!(idle.attention_status.is_empty());
    }

    #[test]
    fn progress_results_split_status_and_error() {
        let shown = |r| {
            let mut s = setting("3");
            let g = s.progress_generation;
            finish_progress(&mut s, g, r);
            assert_eq!(s.progress_phase, ProgressPhase::Idle);
            (s.progress_status, s.progress_error)
        };
        let ok = shown(Ok(ProgressOutcome::Submitted));
        let cleared = shown(Ok(ProgressOutcome::Cleared));
        let unsupported = shown(Ok(ProgressOutcome::Unsupported));
        let failed = shown(Err("x".into()));
        assert!(!ok.0.is_empty() && ok.1.is_empty());
        assert!(!cleared.0.is_empty() && cleared.1.is_empty());
        assert_ne!(ok, cleared);
        for e in [&unsupported, &failed] {
            assert!(e.0.is_empty() && !e.1.is_empty());
            assert!(!e.1.contains("渡しました"));
        }
    }

    #[test]
    fn attention_results_split_status_and_error() {
        let shown = |r| {
            let mut s = requesting();
            let g = s.attention_generation;
            finish_attention(&mut s, g, r);
            assert_eq!(s.attention_phase, AttentionPhase::Idle);
            (s.attention_status, s.attention_error)
        };
        let ok = shown(Ok(AttentionOutcome::Submitted));
        assert!(!ok.0.is_empty() && ok.1.is_empty());
        for e in [
            shown(Ok(AttentionOutcome::Unsupported)),
            shown(Err("x".into())),
        ] {
            assert!(e.0.is_empty() && !e.1.is_empty());
            assert!(!e.1.contains("渡しました"));
        }
    }

    #[test]
    fn attention_fires_only_when_armed_and_unfocused() {
        let mut s = AppProgressAttentionState::default();
        assert_eq!(take_attention_request(&s, false), None);
        arm_attention(&mut s);
        assert_eq!(take_attention_request(&s, true), None);
        assert_eq!(take_attention_request(&s, false), Some(1));
        mark_attention_requesting(&mut s);
        assert_eq!(take_attention_request(&s, false), None);
    }

    #[test]
    fn cancel_is_not_an_error_nor_a_success() {
        let mut s = AppProgressAttentionState::default();
        arm_attention(&mut s);
        cancel_attention(&mut s);
        assert_eq!(s.attention_phase, AttentionPhase::Idle);
        assert!(s.attention_error.is_empty());
        assert_eq!(s.attention_status, ATTENTION_CANCELLED_STATUS);
        assert!(!s.attention_status.contains("渡しました"));
        assert_eq!(take_attention_request(&s, false), None);
        // 取り消し後に届いた旧世代の完了は無視される（Idle）。
        finish_attention(&mut s, 1, Ok(AttentionOutcome::Submitted));
        assert_eq!(s.attention_status, ATTENTION_CANCELLED_STATUS);
    }

    #[test]
    fn busy_screens_drop_buttons() {
        let count = |s: &AppProgressAttentionState| current_screen(s).len();
        let idle = count(&AppProgressAttentionState::default());
        assert!(count(&setting("1")) < idle);
        assert!(count(&requesting()) < idle);
    }
}
