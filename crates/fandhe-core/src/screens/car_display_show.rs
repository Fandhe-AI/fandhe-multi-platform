//! Phase 11（端末機能の拡充ループ、親 #152）型サンプル「車載ディスプレイ」（Issue #1116）。
//! 候補ボタンで車載画面の 1 行テキストを渡し、車載画面が出ているかを表示する 1 画面。
//! `FANDHE_PROXY_APP=car_display_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/
//! registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod car_display_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面
//! ファイルの置き場」節）。`fandhe-egui-proxy` の effect ドライバ（`device/car_display.rs`）が
//! [`take_request`] で要求を観測し、[`finish_set`] / [`finish_state`] で結果を注入する。
//!
//! # 設計の要点
//!
//! - 要求は文言ではなく候補の添字（[`CarDisplayRequest::Set`]）。文言が状態の `Debug` やログに
//!   出る経路を作らないため、状態は `Debug` を derive しない（親 #152「文言をログ・`Debug` に出さない」）
//! - trait に読み戻しは無いので、表示する「最後に受け付けた文字」は画面状態だけが持つ。初期値は未設定
//! - `set` は処理中ボタンを外して高々 1 件。`state` の読み出しは世代で古い完了を捨てる
//! - 車載画面側の文言（タイトル・未設定表示）は Java の strings.xml に置き、ここへは複製しない

use crate::{Screen, WidgetId};

/// 候補ボタンの文言。いずれも 1 行・40 文字以内（`fandhe-device` の `MAX_TEXT_CHARS` と
/// 値を揃える。core は device を参照できないためテストで固定する）。
pub const CANDIDATES: [&str; 3] = ["出発します", "まもなく到着", "休憩中"];

/// effect ドライバが `Unsupported` 相当の失敗を伝えるときの固定文言。
pub const UNAVAILABLE_MARKER: &str = "この環境では車載ディスプレイを使えません";

/// 受付成功の案内。反映は車載画面が出ているときに限るため断りを添える。
pub const ACCEPTED_STATUS: &str = "受け付けました（車載画面に出ているときに反映されます）";

/// 受付失敗の案内の接頭辞。
pub const SET_FAILED_PREFIX: &str = "受け付けに失敗しました: ";

/// 車載画面の文言が未設定のときの表示（車載画面側の固定文言と同じ意味）。
pub const UNSET_TEXT: &str = "未設定";

/// Android の案内（未対応・未読では出さない）。
pub const ANDROID_HINT: &str =
    "Android Auto（DHU）の車載画面でこのアプリを開くと、ここで選んだ文字が出ます";

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarDisplayRequest {
    /// 候補 `CANDIDATES[index]` を車載画面へ渡す。
    Set(usize),
}

/// 状態の読み出し結果（ドライバが画面へ渡す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateOutcome {
    /// 車載画面が出ている。
    Showing,
    /// 出ていない（失敗ではない）。
    NotShowing,
    /// 読み出しに失敗した（`NotShowing` と区別する）。
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Unread,
    Unsupported,
    Showing,
    NotShowing,
    ReadFailed,
}

/// 車載ディスプレイ画面の状態。文言を持たないが、念のため `Debug` は derive しない。
#[derive(Clone)]
pub struct CarDisplayShowState {
    /// 最後に受け付けた候補の添字。
    last_accepted: Option<usize>,
    pending: Option<CarDisplayRequest>,
    /// `pending` を別スレッドへ起動済みか（毎フレーム呼ばれる effect の二重起動防止）。
    dispatched: bool,
    status: Option<String>,
    view: View,
    applied_gen: Option<u64>,
}

impl Default for CarDisplayShowState {
    fn default() -> Self {
        Self {
            last_accepted: None,
            pending: None,
            dispatched: false,
            status: None,
            view: View::Unread,
            applied_gen: None,
        }
    }
}

impl CarDisplayShowState {
    /// 非対応として確定済みか（effect ドライバが読み出しをやめるために使う）。
    pub fn is_unsupported(&self) -> bool {
        self.view == View::Unsupported
    }

    /// 最後に適用した読み出しの世代。
    pub fn applied_gen(&self) -> Option<u64> {
        self.applied_gen
    }
}

/// 候補 `index` を渡す要求を立てる。処理中・範囲外は no-op。
fn request_candidate(state: &mut CarDisplayShowState, index: usize) {
    if state.pending.is_some() || index >= CANDIDATES.len() {
        return;
    }
    state.pending = Some(CarDisplayRequest::Set(index));
    state.status = Some("送信中です…".to_string());
}

fn set_candidate_0(state: &mut CarDisplayShowState) {
    request_candidate(state, 0);
}
fn set_candidate_1(state: &mut CarDisplayShowState) {
    request_candidate(state, 1);
}
fn set_candidate_2(state: &mut CarDisplayShowState) {
    request_candidate(state, 2);
}

/// effect ドライバが未処理の要求を観測する（起動済みは `None`）。
pub fn take_request(state: &CarDisplayShowState) -> Option<CarDisplayRequest> {
    if state.dispatched {
        return None;
    }
    state.pending.clone()
}

/// effect ドライバが要求を別スレッドへ起動した直後に呼ぶ。
pub fn mark_dispatched(state: &mut CarDisplayShowState) {
    state.dispatched = true;
}

/// 受付結果を反映する。成功時のみ表示値を更新し、失敗時は前の値を残す。
/// 要求なしの完了は無視する。
pub fn finish_set(state: &mut CarDisplayShowState, result: Result<(), String>) {
    let Some(CarDisplayRequest::Set(index)) = state.pending.clone() else {
        return;
    };
    state.pending = None;
    state.dispatched = false;
    match result {
        Ok(()) => {
            state.last_accepted = Some(index);
            state.status = Some(ACCEPTED_STATUS.to_string());
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

/// 状態の読み出し結果を反映する。適用済みより古い世代、および非対応確定後の完了は捨てる
/// （遅れて届いた古い結果で新しい失敗表示を上書きしない、非対応を戻さない）。
pub fn finish_state(state: &mut CarDisplayShowState, generation: u64, outcome: StateOutcome) {
    if state.view == View::Unsupported {
        return;
    }
    if matches!(state.applied_gen, Some(prev) if generation < prev) {
        return;
    }
    state.applied_gen = Some(generation);
    state.view = match outcome {
        StateOutcome::Showing => View::Showing,
        StateOutcome::NotShowing => View::NotShowing,
        StateOutcome::Failed => View::ReadFailed,
    };
}

/// この OS では使えないことを反映する。以後の読み出し結果は無視する。
pub fn apply_unsupported(state: &mut CarDisplayShowState) {
    state.view = View::Unsupported;
}

/// 最後に受け付けた文字の表示。
pub fn shown_text(state: &CarDisplayShowState) -> String {
    let text = state.last_accepted.map_or(UNSET_TEXT, |i| CANDIDATES[i]);
    format!("車載画面に渡した文字: {text}")
}

/// 案内文言。
pub fn status_text(state: &CarDisplayShowState) -> String {
    state
        .status
        .clone()
        .unwrap_or_else(|| "ボタンで車載画面に出す文字を選んでください".to_string())
}

/// 車載画面が出ているかの表示。失敗と「出ていない」は別の文言にする。
pub fn display_state_text(state: &CarDisplayShowState) -> String {
    match state.view {
        View::Unread => "車載画面: 読み取り中…",
        View::Unsupported => "車載画面: この OS では未対応です",
        View::Showing => "車載画面: 表示中",
        View::NotShowing => "車載画面: 表示していません",
        View::ReadFailed => "車載画面: 状態を読み出せません",
    }
    .to_string()
}

/// 使い方の案内。未対応・未読では出さない。
pub fn hint_text(state: &CarDisplayShowState) -> String {
    match state.view {
        View::Unread | View::Unsupported => String::new(),
        _ => ANDROID_HINT.to_string(),
    }
}

/// 画面を組み立てる。処理中はボタンを外して二重押しを防ぐ。
pub fn current_screen(state: &CarDisplayShowState) -> Screen<CarDisplayShowState> {
    let screen = Screen::new("車載ディスプレイ")
        .dynamic_text(shown_text)
        .dynamic_text(display_state_text)
        .dynamic_text(status_text)
        .dynamic_text(hint_text);
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

    #[test]
    fn initial_state_has_no_request() {
        assert_eq!(take_request(&CarDisplayShowState::default()), None);
    }

    #[test]
    fn button_sets_request_and_success_updates_value() {
        use crate::click_button;
        let mut s = CarDisplayShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, CANDIDATES[1], &mut s));
        assert_eq!(take_request(&s), Some(CarDisplayRequest::Set(1)));
        finish_set(&mut s, Ok(()));
        assert_eq!(shown_text(&s), "車載画面に渡した文字: まもなく到着");
        assert_eq!(status_text(&s), ACCEPTED_STATUS);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn set_failure_keeps_previous_value() {
        let mut s = CarDisplayShowState::default();
        set_candidate_0(&mut s);
        finish_set(&mut s, Ok(()));
        set_candidate_1(&mut s);
        finish_set(&mut s, Err("boom".to_string()));
        assert_eq!(shown_text(&s), "車載画面に渡した文字: 出発します");
        assert_eq!(status_text(&s), format!("{SET_FAILED_PREFIX}boom"));
    }

    #[test]
    fn unavailable_marker_is_shown_as_is() {
        let mut s = CarDisplayShowState::default();
        set_candidate_0(&mut s);
        finish_set(&mut s, Err(UNAVAILABLE_MARKER.to_string()));
        assert_eq!(status_text(&s), UNAVAILABLE_MARKER);
        assert_eq!(shown_text(&s), "車載画面に渡した文字: 未設定");
    }

    #[test]
    fn completion_without_request_is_ignored() {
        let mut s = CarDisplayShowState::default();
        finish_set(&mut s, Ok(()));
        assert_eq!(shown_text(&s), "車載画面に渡した文字: 未設定");
        assert!(s.status.is_none());
    }

    #[test]
    fn buttons_hidden_while_pending() {
        use crate::click_button;
        let mut s = CarDisplayShowState::default();
        set_candidate_0(&mut s);
        let screen = current_screen(&s);
        assert!(!click_button(&screen, CANDIDATES[0], &mut s));
        // 処理中の再押下は no-op
        set_candidate_2(&mut s);
        assert_eq!(s.pending, Some(CarDisplayRequest::Set(0)));
    }

    #[test]
    fn dispatched_hides_request_until_finished() {
        let mut s = CarDisplayShowState::default();
        set_candidate_0(&mut s);
        mark_dispatched(&mut s);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn failed_and_not_showing_have_distinct_text() {
        let mut a = CarDisplayShowState::default();
        finish_state(&mut a, 1, StateOutcome::Failed);
        let mut b = CarDisplayShowState::default();
        finish_state(&mut b, 1, StateOutcome::NotShowing);
        let mut c = CarDisplayShowState::default();
        finish_state(&mut c, 1, StateOutcome::Showing);
        let texts = [
            display_state_text(&a),
            display_state_text(&b),
            display_state_text(&c),
        ];
        assert_ne!(texts[0], texts[1]);
        assert_ne!(texts[1], texts[2]);
        assert_ne!(texts[0], texts[2]);
    }

    #[test]
    fn stale_generation_does_not_override_newer_failure() {
        let mut s = CarDisplayShowState::default();
        finish_state(&mut s, 2, StateOutcome::Failed);
        finish_state(&mut s, 1, StateOutcome::Showing);
        assert_eq!(display_state_text(&s), "車載画面: 状態を読み出せません");
        assert_eq!(s.applied_gen(), Some(2));
    }

    #[test]
    fn unsupported_is_terminal() {
        let mut s = CarDisplayShowState::default();
        apply_unsupported(&mut s);
        finish_state(&mut s, 5, StateOutcome::Showing);
        assert!(s.is_unsupported());
        assert_eq!(hint_text(&s), "");
    }

    #[test]
    fn candidates_are_single_line_within_40_chars() {
        for c in CANDIDATES {
            assert!(!c.is_empty() && c.chars().count() <= 40);
            assert!(!c.chars().any(|ch| ch.is_control()));
        }
    }
}
