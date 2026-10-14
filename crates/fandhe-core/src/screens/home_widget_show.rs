//! Phase 11（端末機能の拡充ループ、親 #1064）型サンプル「ホーム画面ウィジェット」
//! （Issue #1065）。現在の保存値を表示し、候補ボタンで 1 行テキストを保存する 1 画面。
//! `FANDHE_PROXY_APP=home_widget_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/
//! registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の
//! `pub mod home_widget_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面
//! ファイルの置き場」節）。保存・読み出しは `fandhe-egui-proxy` の effect ドライバ
//! （`device/home_widget.rs`）が [`take_request`] で要求を観測し、[`finish_load`] /
//! [`finish_set`] で結果を注入する。
//!
//! # 要求 → 結果の一巡
//!
//! `credential_vault.rs` と同型。処理中（`pending` あり）はボタンを外して同時に動く
//! 要求を高々 1 件にするため、世代照合は不要。要求の種類が違う完了は無視する。

use crate::{Screen, WidgetId};

/// 候補ボタンの文言。いずれも 1 行・40 文字以内（`fandhe-device` の `MAX_TEXT_CHARS`
/// と値を揃える。core は device を参照できないためテストで固定する）。
pub const CANDIDATES: [&str; 3] = ["おはよう", "会議中", "休憩中"];

/// effect ドライバが `Unsupported` 相当の失敗を伝えるときの固定文言
/// （`credential_vault::UNAVAILABLE_MARKER` と同じ役割の SSOT 文字列）。
pub const UNAVAILABLE_MARKER: &str = "この環境ではウィジェットを使えません";

/// 読み出し失敗の表示（未設定と区別する）。
pub const LOAD_FAILED_TEXT: &str = "読み出しに失敗しました";

/// 保存成功の案内。反映は OS 側で非同期のため断りを添える。
pub const SAVED_STATUS: &str =
    "保存しました（ウィジェットへの反映は OS の都合で遅れることがあります）";

/// 保存失敗の案内の接頭辞。
pub const SET_FAILED_PREFIX: &str = "保存に失敗しました: ";

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeWidgetRequest {
    /// 現在の保存値を読む。
    Load,
    /// テキストを保存する。
    Set(String),
}

/// 表示中の保存値。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shown {
    /// まだ読んでいない。
    Unread,
    /// 未保存。
    Unset,
    /// 保存済みの文字列。
    Text(String),
    /// 読み出しに失敗した。
    LoadFailed,
}

/// ホーム画面ウィジェット画面の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeWidgetShowState {
    shown: Shown,
    /// 未処理の要求。初期状態は `Load`（最初のフレームで現在値を読む）。
    pending: Option<HomeWidgetRequest>,
    /// `pending` を別スレッドへ起動済みか（毎フレーム呼ばれる effect の二重起動防止）。
    dispatched: bool,
    status: Option<String>,
}

impl Default for HomeWidgetShowState {
    fn default() -> Self {
        Self {
            shown: Shown::Unread,
            pending: Some(HomeWidgetRequest::Load),
            dispatched: false,
            status: None,
        }
    }
}

/// 候補 `index` を保存する要求を立てる。処理中・範囲外は no-op。
fn request_candidate(state: &mut HomeWidgetShowState, index: usize) {
    if state.pending.is_some() {
        return;
    }
    if let Some(text) = CANDIDATES.get(index) {
        state.pending = Some(HomeWidgetRequest::Set((*text).to_string()));
        state.status = Some("保存中です…".to_string());
    }
}

fn set_candidate_0(state: &mut HomeWidgetShowState) {
    request_candidate(state, 0);
}
fn set_candidate_1(state: &mut HomeWidgetShowState) {
    request_candidate(state, 1);
}
fn set_candidate_2(state: &mut HomeWidgetShowState) {
    request_candidate(state, 2);
}

/// effect ドライバが未処理の要求を観測する（起動済みは `None`）。
pub fn take_request(state: &HomeWidgetShowState) -> Option<HomeWidgetRequest> {
    if state.dispatched {
        return None;
    }
    state.pending.clone()
}

/// effect ドライバが要求を別スレッドへ起動した直後に呼ぶ。
pub fn mark_dispatched(state: &mut HomeWidgetShowState) {
    state.dispatched = true;
}

/// 読み出し結果を反映する。`Load` 以外の要求中・要求なしの完了は無視する。
pub fn finish_load(state: &mut HomeWidgetShowState, result: Result<Option<String>, String>) {
    if state.pending != Some(HomeWidgetRequest::Load) {
        return;
    }
    state.pending = None;
    state.dispatched = false;
    match result {
        Ok(Some(text)) => {
            state.shown = Shown::Text(text);
            state.status = None;
        }
        Ok(None) => {
            state.shown = Shown::Unset;
            state.status = None;
        }
        Err(message) => {
            state.shown = Shown::LoadFailed;
            state.status = Some(error_status(&message));
        }
    }
}

/// 保存結果を反映する。成功時のみ表示値を更新し、失敗時は前の値を残す。
/// `Set` 以外の要求中・要求なしの完了は無視する。
pub fn finish_set(state: &mut HomeWidgetShowState, result: Result<(), String>) {
    let Some(HomeWidgetRequest::Set(text)) = state.pending.clone() else {
        return;
    };
    state.pending = None;
    state.dispatched = false;
    match result {
        Ok(()) => {
            state.shown = Shown::Text(text);
            state.status = Some(SAVED_STATUS.to_string());
        }
        Err(message) => {
            state.status = Some(if message == UNAVAILABLE_MARKER {
                message
            } else {
                format!("{SET_FAILED_PREFIX}{message}")
            });
        }
    }
}

fn error_status(message: &str) -> String {
    if message == UNAVAILABLE_MARKER {
        message.to_string()
    } else {
        format!("{LOAD_FAILED_TEXT}: {message}")
    }
}

/// 現在値の表示（`DynamicText` の `get`）。
pub fn shown_text(state: &HomeWidgetShowState) -> String {
    match &state.shown {
        Shown::Unread => "ウィジェットの文字: 読み込み中…".to_string(),
        Shown::Unset => "ウィジェットの文字: 未設定".to_string(),
        Shown::Text(text) => format!("ウィジェットの文字: {text}"),
        Shown::LoadFailed => LOAD_FAILED_TEXT.to_string(),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &HomeWidgetShowState) -> String {
    state
        .status
        .clone()
        .unwrap_or_else(|| "ボタンで表示する文字を選んでください".to_string())
}

/// 画面を組み立てる。処理中はボタンを外して二重押しを防ぐ。
pub fn current_screen(state: &HomeWidgetShowState) -> Screen<HomeWidgetShowState> {
    let screen = Screen::new("ホーム画面ウィジェット")
        .dynamic_text(shown_text)
        .dynamic_text(status_text);
    if state.pending.is_some() {
        return screen;
    }
    screen
        .button(WidgetId::from_raw(1), CANDIDATES[0], set_candidate_0)
        .button(WidgetId::from_raw(2), CANDIDATES[1], set_candidate_1)
        .button(WidgetId::from_raw(3), CANDIDATES[2], set_candidate_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> HomeWidgetShowState {
        let mut s = HomeWidgetShowState::default();
        finish_load(&mut s, Ok(None));
        s
    }

    #[test]
    fn initial_state_requests_load() {
        let s = HomeWidgetShowState::default();
        assert_eq!(take_request(&s), Some(HomeWidgetRequest::Load));
    }

    #[test]
    fn load_none_shows_unset_and_text_shows_value() {
        let mut s = HomeWidgetShowState::default();
        finish_load(&mut s, Ok(None));
        assert_eq!(shown_text(&s), "ウィジェットの文字: 未設定");
        let mut s = HomeWidgetShowState::default();
        finish_load(&mut s, Ok(Some("会議中".to_string())));
        assert_eq!(shown_text(&s), "ウィジェットの文字: 会議中");
    }

    #[test]
    fn load_failure_is_distinct_from_unset() {
        let mut s = HomeWidgetShowState::default();
        finish_load(&mut s, Err("x".to_string()));
        assert_eq!(shown_text(&s), LOAD_FAILED_TEXT);
        assert_ne!(shown_text(&s), shown_text(&idle()));
    }

    #[test]
    fn button_sets_request_and_roundtrip_updates_value() {
        let mut s = idle();
        set_candidate_1(&mut s);
        assert_eq!(
            take_request(&s),
            Some(HomeWidgetRequest::Set("会議中".to_string()))
        );
        finish_set(&mut s, Ok(()));
        assert_eq!(shown_text(&s), "ウィジェットの文字: 会議中");
        assert_eq!(status_text(&s), SAVED_STATUS);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn set_failure_keeps_previous_value() {
        let mut s = idle();
        set_candidate_0(&mut s);
        finish_set(&mut s, Err("boom".to_string()));
        assert_eq!(shown_text(&s), "ウィジェットの文字: 未設定");
        assert_eq!(status_text(&s), format!("{SET_FAILED_PREFIX}boom"));
    }

    #[test]
    fn unavailable_marker_is_shown_as_is() {
        let mut s = idle();
        set_candidate_0(&mut s);
        finish_set(&mut s, Err(UNAVAILABLE_MARKER.to_string()));
        assert_eq!(status_text(&s), UNAVAILABLE_MARKER);
    }

    #[test]
    fn buttons_hidden_while_pending_and_work_when_idle() {
        use crate::click_button;
        let mut s = HomeWidgetShowState::default();
        let screen = current_screen(&s);
        assert!(!click_button(&screen, "会議中", &mut s));
        let mut idle_state = idle();
        let screen = current_screen(&idle_state);
        assert!(click_button(&screen, "会議中", &mut idle_state));
        // 処理中の再押下は no-op
        set_candidate_0(&mut idle_state);
        assert_eq!(
            idle_state.pending,
            Some(HomeWidgetRequest::Set("会議中".to_string()))
        );
    }

    #[test]
    fn mismatched_or_absent_completion_is_ignored() {
        let mut s = idle();
        finish_set(&mut s, Ok(()));
        finish_load(&mut s, Ok(Some("x".to_string())));
        assert_eq!(shown_text(&s), "ウィジェットの文字: 未設定");
        set_candidate_0(&mut s);
        finish_load(&mut s, Ok(Some("x".to_string())));
        assert!(s.pending.is_some());
    }

    #[test]
    fn dispatched_hides_request_until_finished() {
        let mut s = HomeWidgetShowState::default();
        mark_dispatched(&mut s);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn candidates_are_single_line_within_40_chars() {
        for c in CANDIDATES {
            assert!(c.chars().count() <= 40);
            assert!(!c.chars().any(|ch| ch.is_control()));
        }
    }
}
