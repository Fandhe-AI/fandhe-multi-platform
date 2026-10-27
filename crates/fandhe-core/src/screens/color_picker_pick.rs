//! Phase 11 の拡充ループ型サンプル「システムの色選択ダイアログ」（親 #184、Issue #941）。
//! 「色を選ぶ」ボタン → OS の色選択 UI → 選んだ色を見本と `#rrggbb` で出す 1 画面。
//! `FANDHE_PROXY_APP=color_picker` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod color_picker_pick;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。橋渡し層が `DeviceError` を
//! [`ColorPickerOutcome`] へ畳んで渡す。macOS・Windows・iOS は egui-proxy、Web は `fandhe-web-dom`
//! が同じ型・同じ文言定数を使う。
//!
//! # 成功・取り消し・非対応・失敗を混同しない
//!
//! [`ColorPickerOutcome::Picked`] だけが [`ColorPickerState::picked`] を埋める。それ以外は必ず
//! `None` に戻す。取り消しは `status`（通常色）、非対応・失敗は `error`（`TextRole::Error`）。
//! 進行状態・世代照合は `file_pick` と同じ（[`PickPhase`] を再利用する）。
//!
//! # 画面の種類
//!
//! [`current_screen`] はネイティブ用（OS のダイアログが閉じ方を持つ）。[`web_screen`] は選択中に
//! 「やめる」ボタンを出す（`<input type="color">` は取り消しの通知が無く、画面側で抜けられるようにする）。
//! [`abandon_pick`] は世代を進めるので、やめた後に元の要求の完了が届いても反映されない。

use crate::token::ColorValue;
use crate::ui::CanvasWidget;
use crate::{Measurement, Node, PaintCommand, Screen, WidgetId};

pub use super::file_pick::PickPhase;

/// 「色を選ぶ」ボタンの `WidgetId`。
const PICK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「やめる」ボタンの `WidgetId`（Web のみ）。
const ABANDON_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 色見本の `WidgetId`。
const SWATCH_ID: WidgetId = WidgetId::from_raw(3);

/// 色見本の幅。
pub const SWATCH_W: f32 = 160.0;
/// 色見本の高さ。
pub const SWATCH_H: f32 = 80.0;

/// 未操作時の案内。
pub const IDLE_HINT: &str =
    "「色を選ぶ」を押すと OS の色選択が開きます。閉じると選んだ色が決まります";
/// ボタンの文言（Web の DOM ボタンもこの定数を使う）。
pub const BUTTON_LABEL: &str = "色を選ぶ";
/// 選択中の案内。
pub const PICKING_STATUS: &str = "色のパネルを表示中です。閉じると色が決まります";
/// 色を選ばずに閉じたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "色を選ばずに閉じました";
/// 「やめる」ボタンの文言（Web のみ）。
pub const ABANDON_LABEL: &str = "やめる";
/// やめたときの案内（OS の結果ではなく画面側の操作）。
pub const ABANDONED_STATUS: &str = "選ぶのをやめました";
/// 非対応環境の案内。
pub const UNSUPPORTED_TEXT: &str = "この環境では色選択を使えません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 橋渡し層が画面へ渡す結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorPickerOutcome {
    /// 選ばれた色（0xRRGGBB。範囲の保証は橋渡し層が持つ）。
    Picked(u32),
    /// 色を選ばずに閉じた。
    Cancelled,
    /// この環境では使えない。
    Unsupported,
    /// 失敗（理由は内部識別子のみで、色は含まない）。
    Failed(String),
}

/// 色選択画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColorPickerState {
    /// 操作の進行状態。
    pub phase: PickPhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 選ばれた色（0xRRGGBB）。成功の唯一の根拠。
    pub picked: Option<u32>,
    /// 取り消し・やめたときの案内文言（通常色）。
    pub status: String,
    /// 非対応・失敗の理由（`TextRole::Error`）。
    pub error: String,
}

/// ボタンのハンドラ。`Idle` 以外は no-op。前の色は新しい要求で消す。
pub fn request_pick(state: &mut ColorPickerState) {
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
pub fn take_request(state: &ColorPickerState) -> Option<u64> {
    (state.phase == PickPhase::Requested).then_some(state.generation)
}

/// 要求を処理中（`Picking`）へ進める。`Requested` 以外では何もしない。
pub fn mark_picking(state: &mut ColorPickerState) {
    if state.phase == PickPhase::Requested {
        state.phase = PickPhase::Picking;
    }
}

/// 結果を反映する。`Idle` のとき・世代が一致しないときは無視する。
pub fn finish_pick(state: &mut ColorPickerState, generation: u64, outcome: ColorPickerOutcome) {
    if state.phase == PickPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PickPhase::Idle;
    state.picked = None;
    state.status = String::new();
    state.error = String::new();
    match outcome {
        ColorPickerOutcome::Picked(rgb) => state.picked = Some(rgb),
        ColorPickerOutcome::Cancelled => state.status = CANCELLED_STATUS.to_string(),
        ColorPickerOutcome::Unsupported => state.error = UNSUPPORTED_TEXT.to_string(),
        ColorPickerOutcome::Failed(message) => {
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 利用者が選択中の要求をやめる（Web 用）。`Requested`・`Picking` から `Idle` へ戻し、
/// 世代を進めて元の要求の完了を無効にする。それ以外では何もしない。
pub fn abandon_pick(state: &mut ColorPickerState) {
    if !matches!(state.phase, PickPhase::Requested | PickPhase::Picking) {
        return;
    }
    state.phase = PickPhase::Idle;
    state.generation = state.generation.wrapping_add(1);
    state.picked = None;
    state.error = String::new();
    state.status = ABANDONED_STATUS.to_string();
}

/// 選んだ色の表示文言（`#rrggbb` 小文字）。未選択なら空文字列。
pub fn picked_text(state: &ColorPickerState) -> String {
    state
        .picked
        .map(|c| format!("選んだ色: #{c:06x}"))
        .unwrap_or_default()
}

/// 案内文言。選択中は [`PICKING_STATUS`]、それ以外は取り消し・やめた案内。
pub fn status_text(state: &ColorPickerState) -> String {
    if state.phase == PickPhase::Picking {
        return PICKING_STATUS.to_string();
    }
    state.status.clone()
}

/// 失敗時の案内文言。
pub fn error_text(state: &ColorPickerState) -> String {
    state.error.clone()
}

/// 色見本の描画コマンド。選んだときだけ塗り、それ以外は空（代わりの色を描かない）。
pub fn swatch_commands(state: &ColorPickerState) -> Vec<PaintCommand> {
    match state.picked {
        Some(c) => vec![PaintCommand::StyledRect {
            fill: ColorValue::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8),
        }],
        None => Vec::new(),
    }
}

fn base_screen() -> Screen<ColorPickerState> {
    Screen::new("システムの色選択")
        .text(IDLE_HINT)
        .with_node(Node::Widget(Box::new(CanvasWidget::bound(
            SWATCH_ID,
            "選んだ色の見本",
            Measurement {
                width: SWATCH_W,
                height: SWATCH_H,
            },
            swatch_commands,
        ))))
        .dynamic_text(picked_text)
        .dynamic_text(status_text)
        .error_text(error_text)
}

/// ネイティブ用の画面。ボタンは `Idle` のときだけ出す。
pub fn current_screen(state: &ColorPickerState) -> Screen<ColorPickerState> {
    let screen = base_screen();
    if state.phase != PickPhase::Idle {
        return screen;
    }
    screen.button(PICK_BUTTON_ID, BUTTON_LABEL, request_pick)
}

/// Web 用の画面。選択中は「やめる」ボタンを出す。
pub fn web_screen(state: &ColorPickerState) -> Screen<ColorPickerState> {
    let screen = base_screen();
    match state.phase {
        PickPhase::Idle => screen.button(PICK_BUTTON_ID, BUTTON_LABEL, request_pick),
        _ => screen.button(ABANDON_BUTTON_ID, ABANDON_LABEL, abandon_pick),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picking() -> ColorPickerState {
        let mut s = ColorPickerState::default();
        request_pick(&mut s);
        mark_picking(&mut s);
        s
    }

    #[test]
    fn success_sets_picked_only() {
        let mut s = picking();
        assert_eq!(status_text(&s), PICKING_STATUS);
        finish_pick(&mut s, 1, ColorPickerOutcome::Picked(0xAABBCC));
        assert_eq!(s.phase, PickPhase::Idle);
        assert_eq!(picked_text(&s), "選んだ色: #aabbcc");
        assert!(s.status.is_empty() && s.error.is_empty());
    }

    #[test]
    fn non_success_outcomes_never_leave_a_color() {
        let cases = [
            (ColorPickerOutcome::Cancelled, false),
            (ColorPickerOutcome::Unsupported, true),
            (ColorPickerOutcome::Failed("x".into()), true),
        ];
        let mut texts = Vec::new();
        for (outcome, is_error) in cases {
            let mut s = picking();
            s.picked = Some(0);
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
        finish_pick(&mut s, 99, ColorPickerOutcome::Picked(0xAABBCC));
        assert_eq!(s.phase, PickPhase::Picking);
        assert!(s.picked.is_none());
        let mut idle = ColorPickerState::default();
        finish_pick(&mut idle, 0, ColorPickerOutcome::Picked(0xAABBCC));
        assert!(idle.picked.is_none());
    }

    #[test]
    fn abandon_invalidates_the_original_request() {
        let mut s = picking();
        abandon_pick(&mut s);
        assert_eq!(s.phase, PickPhase::Idle);
        assert_eq!(s.status, ABANDONED_STATUS);
        finish_pick(&mut s, 1, ColorPickerOutcome::Picked(0xAABBCC));
        assert!(s.picked.is_none());
        // Idle での abandon は何もしない。
        let before = s.clone();
        abandon_pick(&mut s);
        assert_eq!(s, before);
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut s = picking();
        request_pick(&mut s);
        assert_eq!(s.phase, PickPhase::Picking);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn new_request_clears_previous_color() {
        let mut s = ColorPickerState {
            picked: Some(0xAABBCC),
            ..Default::default()
        };
        request_pick(&mut s);
        assert!(s.picked.is_none());
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn buttons_follow_phase() {
        let idle = ColorPickerState::default();
        let busy = picking();
        // ネイティブ: 選択中はボタン無し。Web: 選択中は「やめる」が出る。
        assert_eq!(current_screen(&idle).len(), current_screen(&busy).len() + 1);
        assert_eq!(web_screen(&idle).len(), web_screen(&busy).len());
        assert_eq!(web_screen(&busy).len(), current_screen(&busy).len() + 1);
    }

    #[test]
    fn swatch_only_when_picked() {
        let mut s = picking();
        assert!(swatch_commands(&s).is_empty());
        finish_pick(&mut s, 1, ColorPickerOutcome::Picked(0x112233));
        assert_eq!(swatch_commands(&s).len(), 1);
        finish_pick(&mut s, 1, ColorPickerOutcome::Cancelled);
        request_pick(&mut s);
        assert!(swatch_commands(&s).is_empty());
    }
}
