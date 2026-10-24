//! Phase 11 の拡充ループ型サンプル「画面のスポイト」（親 #148、Issue #762）。
//! 「スポイトで色を拾う」ボタン → OS のスポイト → 選んだ色を `#rrggbb` で出す 1 画面。
//! `FANDHE_PROXY_APP=eye_dropper` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod eye_dropper_pick;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。橋渡し層が `DeviceError` を
//! [`EyeDropperOutcome`] へ畳んで渡す。macOS は egui-proxy、Web は `fandhe-web-dom`（#763）が
//! 同じ型・同じ文言定数を使う。
//!
//! # 成功・取り消し・非対応・失敗を混同しない
//!
//! [`EyeDropperOutcome::Picked`] だけが [`EyeDropperState::picked`] を埋める。それ以外は必ず
//! `None` に戻す。取り消しは `status`（通常色）、非対応・失敗は `error`（`TextRole::Error`）。
//! 進行状態・世代照合は `file_pick` と同じ（[`PickPhase`] を再利用する）。

use crate::{Screen, WidgetId};

pub use super::file_pick::PickPhase;

/// 「スポイトで色を拾う」ボタンの `WidgetId`。
const PICK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 未操作時の案内。
pub const IDLE_HINT: &str = "「スポイトで色を拾う」を押して、画面上の 1 点をクリックすると色を拾います（macOS・デスクトップの Chrome / Edge）";
/// ボタンの文言（Web の DOM ボタンもこの定数を使う）。
pub const BUTTON_LABEL: &str = "スポイトで色を拾う";
/// スポイト表示中の案内。
pub const PICKING_STATUS: &str = "スポイトを表示中です。クリックで決定、Esc で取り消し";
/// 取り消されたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "色を拾いませんでした";
/// 非対応環境の案内。
pub const UNSUPPORTED_TEXT: &str = "この環境では画面の色を拾えません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 橋渡し層が画面へ渡す結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EyeDropperOutcome {
    /// 選ばれた色（正規化済みの `#rrggbb`。保証は橋渡し層が持つ）。
    Picked(String),
    /// 利用者が取り消した。
    Cancelled,
    /// この環境では拾えない。
    Unsupported,
    /// 失敗（理由は内部識別子のみで、色は含まない）。
    Failed(String),
}

/// スポイト画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EyeDropperState {
    /// 操作の進行状態。
    pub phase: PickPhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 選ばれた色。成功の唯一の根拠。
    pub picked: Option<String>,
    /// 取り消しの案内文言（通常色）。
    pub status: String,
    /// 非対応・失敗の理由（`TextRole::Error`）。
    pub error: String,
}

/// ボタンのハンドラ。`Idle` 以外は no-op。前の色は新しい要求で消す。
pub fn request_pick(state: &mut EyeDropperState) {
    if state.phase != PickPhase::Idle {
        return;
    }
    state.phase = PickPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.picked = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &EyeDropperState) -> Option<u64> {
    (state.phase == PickPhase::Requested).then_some(state.generation)
}

/// 要求を処理中（`Picking`）へ進める。`Requested` 以外では何もしない。
pub fn mark_picking(state: &mut EyeDropperState) {
    if state.phase == PickPhase::Requested {
        state.phase = PickPhase::Picking;
    }
}

/// 結果を反映する。`Idle` のとき・世代が一致しないときは無視する。
pub fn finish_pick(state: &mut EyeDropperState, generation: u64, outcome: EyeDropperOutcome) {
    if state.phase == PickPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PickPhase::Idle;
    state.picked = None;
    state.status = String::new();
    state.error = String::new();
    match outcome {
        EyeDropperOutcome::Picked(hex) => state.picked = Some(hex),
        EyeDropperOutcome::Cancelled => state.status = CANCELLED_STATUS.to_string(),
        EyeDropperOutcome::Unsupported => state.error = UNSUPPORTED_TEXT.to_string(),
        EyeDropperOutcome::Failed(message) => {
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 選んだ色の表示文言（`DynamicText` の `get`）。未選択なら空文字列。
pub fn picked_text(state: &EyeDropperState) -> String {
    state
        .picked
        .as_ref()
        .map(|c| format!("選んだ色: {c}"))
        .unwrap_or_default()
}

/// 案内文言。表示中は [`PICKING_STATUS`]、取り消し後は [`CANCELLED_STATUS`]。
pub fn status_text(state: &EyeDropperState) -> String {
    if state.phase == PickPhase::Picking {
        return PICKING_STATUS.to_string();
    }
    state.status.clone()
}

/// 失敗時の案内文言。
pub fn error_text(state: &EyeDropperState) -> String {
    state.error.clone()
}

/// スポイト画面を組み立てる。ボタンは `Idle` のときだけ出す。
pub fn current_screen(state: &EyeDropperState) -> Screen<EyeDropperState> {
    let screen = Screen::new("画面のスポイト")
        .text(IDLE_HINT)
        .dynamic_text(picked_text)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != PickPhase::Idle {
        return screen;
    }
    screen.button(PICK_BUTTON_ID, BUTTON_LABEL, request_pick)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picking() -> EyeDropperState {
        let mut s = EyeDropperState::default();
        request_pick(&mut s);
        mark_picking(&mut s);
        s
    }

    #[test]
    fn success_sets_picked_only() {
        let mut s = picking();
        assert_eq!(status_text(&s), PICKING_STATUS);
        finish_pick(&mut s, 1, EyeDropperOutcome::Picked("#aabbcc".into()));
        assert_eq!(s.phase, PickPhase::Idle);
        assert_eq!(picked_text(&s), "選んだ色: #aabbcc");
        assert!(s.status.is_empty() && s.error.is_empty());
    }

    #[test]
    fn non_success_outcomes_never_leave_a_color() {
        let cases = [
            (EyeDropperOutcome::Cancelled, false),
            (EyeDropperOutcome::Unsupported, true),
            (EyeDropperOutcome::Failed("x".into()), true),
        ];
        let mut texts = Vec::new();
        for (outcome, is_error) in cases {
            let mut s = picking();
            s.picked = Some("#000000".into());
            finish_pick(&mut s, 1, outcome);
            assert!(s.picked.is_none());
            assert_eq!(s.error.is_empty(), !is_error);
            assert_eq!(s.status.is_empty(), is_error);
            texts.push(format!("{}{}", s.status, s.error));
        }
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), 3, "文言が区別される");
    }

    #[test]
    fn stale_generation_and_idle_completion_are_ignored() {
        let mut s = picking();
        finish_pick(&mut s, 99, EyeDropperOutcome::Picked("#aabbcc".into()));
        assert_eq!(s.phase, PickPhase::Picking);
        assert!(s.picked.is_none());
        let mut idle = EyeDropperState::default();
        finish_pick(&mut idle, 0, EyeDropperOutcome::Picked("#aabbcc".into()));
        assert!(idle.picked.is_none());
    }

    #[test]
    fn new_request_clears_previous_color() {
        let mut s = EyeDropperState {
            picked: Some("#aabbcc".into()),
            ..Default::default()
        };
        request_pick(&mut s);
        assert!(s.picked.is_none());
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut s = picking();
        request_pick(&mut s);
        assert_eq!(s.phase, PickPhase::Picking);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn button_only_when_idle() {
        let idle = EyeDropperState::default();
        let busy = picking();
        assert_eq!(current_screen(&idle).len(), current_screen(&busy).len() + 1);
    }
}
