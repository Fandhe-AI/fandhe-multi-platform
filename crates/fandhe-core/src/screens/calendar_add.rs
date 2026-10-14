//! Phase 11（端末機能の拡充ループ、親 #1118）型サンプル「予定の追加」（Issue #1224）。
//! 件名・開始・終了の入力欄と「予定を追加」ボタンだけの 1 画面。
//! `FANDHE_PROXY_APP=calendar_add` で選ばれ、`crates/fandhe-egui-proxy/src/screens/
//! registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod calendar_add;`
//! と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。入力の検査と OS 呼び出しは
//! `fandhe-egui-proxy` の effect ドライバ（`device/calendar.rs`）が [`take_request`] で
//! 要求を観測して行い、[`finish_add`] で結果を注入する。本画面は二重押し防止だけを持つ。
//!
//! # 結果を混同しない理由
//!
//! 「OS の画面へ渡しただけ」「キャンセル」「権限拒否」を黙って「予定を追加しました」に
//! 丸めると、利用者は保存されたと誤解する。保存済みの文言は [`Ok(AddOutcome::Saved)`]
//! のときだけ出す。OS が判定した正常な結果は `status`、入力不正・失敗は
//! `error`（`TextRole::Error`）へ書き分ける（`biometric_check` と同型）。
//!
//! 世代番号（`generation`）で、新しい要求に上書きされた古い完了や `Idle` で届いた完了を
//! 無視する（`biometric_check` と同じ判断）。

use crate::{Screen, WidgetId};

/// 「予定を追加」ボタンの `WidgetId`。
const ADD_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 保存できたときの案内。
pub const SAVED_STATUS: &str = "予定を追加しました";
/// OS の画面へ渡しただけのときの案内（「追加しました」を含めない）。
pub const HANDED_STATUS: &str =
    "カレンダーの画面へ渡しました（保存されたかはカレンダーで確認してください）";
/// キャンセルされたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 権限が無いときの案内。
pub const DENIED_STATUS: &str =
    "カレンダーへの追加が許可されていません（システム設定で変更できます）";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では予定を追加できません";
/// 入力不正の接頭辞。
pub const INVALID_PREFIX: &str = "入力を確認してください: ";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 追加操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_adding`] 済み）。
    Adding,
}

/// OS が判定した結果（`fandhe_device::calendar::AddEventOutcome` と権限拒否・非対応を
/// 合わせたもの。写像は `fandhe-egui-proxy` 側の責務）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    /// 保存が確定した。
    Saved,
    /// OS の編集画面へ渡しただけ。
    HandedToOs,
    /// 利用者がキャンセルした。
    Cancelled,
    /// 権限拒否・制限。
    Denied,
    /// この環境では非対応。
    Unsupported,
}

/// 失敗（画面には固定文言だけを出す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddFailure {
    /// 入力不正（OS へは渡していない）。
    Invalid(String),
    /// 内部失敗。
    Failed(String),
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRequest {
    /// [`finish_add`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 件名（未検査の入力そのまま）。
    pub title: String,
    /// 開始（`YYYY-MM-DD HH:MM`、未検査）。
    pub start: String,
    /// 終了（同上）。
    pub end: String,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarAddState {
    /// 件名の入力値。
    pub title: String,
    /// 開始の入力値。
    pub start: String,
    /// 終了の入力値。
    pub end: String,
    /// 進行状態。
    pub phase: AddPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 入力不正・失敗（`TextRole::Error`）。
    pub error: String,
}

impl Default for CalendarAddState {
    fn default() -> Self {
        Self {
            title: String::new(),
            // 目視しやすいよう見本値を入れておく。
            start: "2026-10-10 10:00".to_string(),
            end: "2026-10-10 11:00".to_string(),
            phase: AddPhase::Idle,
            generation: 0,
            status: String::new(),
            error: String::new(),
        }
    }
}

fn get_title(s: &CalendarAddState) -> String {
    s.title.clone()
}
fn set_title(s: &mut CalendarAddState, v: String) {
    s.title = v;
}
fn get_start(s: &CalendarAddState) -> String {
    s.start.clone()
}
fn set_start(s: &mut CalendarAddState, v: String) {
    s.start = v;
}
fn get_end(s: &CalendarAddState) -> String {
    s.end.clone()
}
fn set_end(s: &mut CalendarAddState, v: String) {
    s.end = v;
}

/// 「予定を追加」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn add(state: &mut CalendarAddState) {
    if state.phase != AddPhase::Idle {
        return;
    }
    state.phase = AddPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &CalendarAddState) -> Option<AddRequest> {
    (state.phase == AddPhase::Requested).then(|| AddRequest {
        generation: state.generation,
        title: state.title.clone(),
        start: state.start.clone(),
        end: state.end.clone(),
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_adding(state: &mut CalendarAddState) {
    if state.phase == AddPhase::Requested {
        state.phase = AddPhase::Adding;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_add(
    state: &mut CalendarAddState,
    generation: u64,
    result: Result<AddOutcome, AddFailure>,
) {
    if state.phase == AddPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = AddPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(AddOutcome::Saved) => state.status = SAVED_STATUS.to_string(),
        Ok(AddOutcome::HandedToOs) => state.status = HANDED_STATUS.to_string(),
        Ok(AddOutcome::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Ok(AddOutcome::Denied) => state.status = DENIED_STATUS.to_string(),
        Ok(AddOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(AddFailure::Invalid(m)) => state.error = format!("{INVALID_PREFIX}{m}"),
        Err(AddFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &CalendarAddState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &CalendarAddState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外して二重押しを防ぐ。
pub fn current_screen(state: &CalendarAddState) -> Screen<CalendarAddState> {
    let screen = Screen::new("予定の追加")
        .text_field("件名", get_title, set_title)
        .text_field("開始（YYYY-MM-DD HH:MM）", get_start, set_start)
        .text_field("終了（YYYY-MM-DD HH:MM）", get_end, set_end)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != AddPhase::Idle {
        return screen;
    }
    screen.button(ADD_BUTTON_ID, "予定を追加", add)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adding() -> CalendarAddState {
        let mut s = CalendarAddState::default();
        add(&mut s);
        mark_adding(&mut s);
        s
    }

    fn shown(r: Result<AddOutcome, AddFailure>) -> (String, String) {
        let mut s = adding();
        let g = s.generation;
        finish_add(&mut s, g, r);
        assert_eq!(s.phase, AddPhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn seven_results_are_distinct_and_only_saved_says_added() {
        let all = [
            shown(Ok(AddOutcome::Saved)),
            shown(Ok(AddOutcome::HandedToOs)),
            shown(Ok(AddOutcome::Cancelled)),
            shown(Ok(AddOutcome::Denied)),
            shown(Ok(AddOutcome::Unsupported)),
            shown(Err(AddFailure::Invalid("x".into()))),
            shown(Err(AddFailure::Failed("x".into()))),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
            let said_added = format!("{}{}", a.0, a.1).contains(SAVED_STATUS);
            assert_eq!(said_added, i == 0, "index {i}");
        }
        assert!(!HANDED_STATUS.contains("追加しました"));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = adding();
        let g = s.generation + 1;
        finish_add(&mut s, g, Ok(AddOutcome::Saved));
        assert_eq!(s.phase, AddPhase::Adding);
        assert!(s.status.is_empty());

        let mut idle = CalendarAddState::default();
        finish_add(&mut idle, 0, Ok(AddOutcome::Saved));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn add_is_noop_while_in_flight_and_clears_previous() {
        let mut s = adding();
        let g = s.generation;
        add(&mut s);
        assert_eq!((s.phase, s.generation), (AddPhase::Adding, g));

        let mut s = CalendarAddState {
            status: SAVED_STATUS.into(),
            error: "e".into(),
            ..Default::default()
        };
        add(&mut s);
        assert!(s.status.is_empty() && s.error.is_empty());
        assert_eq!(s.phase, AddPhase::Requested);
    }

    #[test]
    fn take_request_only_when_requested() {
        let mut s = CalendarAddState::default();
        assert!(take_request(&s).is_none());
        add(&mut s);
        let r = take_request(&s).unwrap();
        assert_eq!(r.generation, s.generation);
        mark_adding(&mut s);
        assert!(take_request(&s).is_none());
    }
}
