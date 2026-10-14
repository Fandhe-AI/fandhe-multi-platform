//! Phase 11 の拡充ループ（親 #932）型サンプル「ゲームコントローラ」（Issue #933）。
//! `FANDHE_PROXY_APP=gamepad_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod gamepad_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。ゲームコントローラの実読み取りは
//! 本モジュール外（`fandhe-egui-proxy` の effect ドライバ、`device/gamepad.rs`）が
//! `fandhe_device::gamepad::Gamepad` を pull し、[`apply_connected`] /
//! [`apply_disconnected`] / [`apply_unsupported`] を呼んで結果を注入する。
//!
//! 玉の絵・物理は [`crate::screens::tilt_ball`] の公開 API（[`tilt_ball::step`] /
//! [`tilt_ball::ball_commands`]）をそのまま再利用する（複製しない）。左スティックの
//! `(x, y)` を `tilt_ball::TiltBallState::tilt_x`/`tilt_y` 相当の入力として渡す。
//!
//! # 3 状態を区別する理由（Issue #859 の再発防止）
//!
//! 「接続中」「未接続（代替入力が効いている）」「非対応（代替入力が効いている）」を
//! 同じ文言で潰さない。代替入力（スライダー・チェックボックス）は実装が非対応・
//! 未接続のときだけ効き、実コントローラが読めているときは上書きしない
//! （effect ドライバ側の選択ロジックの契約。本モジュールは注入された `view` を
//! そのまま表示するだけで、選択自体は行わない）。

use crate::screens::tilt_ball::{self, TiltBallState};
use crate::ui::CanvasWidget;
use crate::{Measurement, Node, PaintCommand, Screen, WidgetId};

const CANVAS_ID: WidgetId = WidgetId::from_raw(1);
const ALT_X_SLIDER_ID: WidgetId = WidgetId::from_raw(2);
const ALT_Y_SLIDER_ID: WidgetId = WidgetId::from_raw(3);
const ALT_BUTTON_CHECKBOX_ID: WidgetId = WidgetId::from_raw(4);

/// 「接続中」の案内ラベル。
pub const CONNECTED_LABEL: &str = "接続中";
/// 「未接続（代替入力が効いている）」の案内ラベル。`CONNECTED_LABEL` と別文言にする
/// （モジュール doc の再発防止、Issue #859）。
pub const DISCONNECTED_ALT_LABEL: &str = "未接続（代替入力）";
/// 「非対応（代替入力が効いている）」の案内ラベル。同じく別文言。
pub const UNSUPPORTED_ALT_LABEL: &str = "非対応（代替入力）";
/// ボタン未押下時の表示。
const NO_BUTTON_TEXT: &str = "なし";
/// 代替入力チェックボックスが表す仮想ボタン名。
const ALT_BUTTON_LABEL: &str = "代替ボタン";
/// 「傾き無し」とみなす代替入力のデッドゾーン（`fandhe-egui-proxy::device::gamepad::
/// STICK_EPSILON` と同じ値）。[`apply_alt_frame`] が [`tilt_ball::step`] に渡す前に
/// この閾値以下を 0.0 として畳む。effect ドライバ側は接続時のみこの畳み込みを行い
/// [`apply_disconnected`]/[`apply_unsupported`] へは生の `alt_x`/`alt_y` をそのまま
/// 渡す契約（`gamepad.rs::drive` 参照）のため、ここで畳まないとスライダーの残留値が
/// 玉に力を与え続け、静止しきらず effect ドライバの skip-send 判定が永久に成立しない
/// （Cursor Bugbot 指摘 Medium、PR #937）。
const ALT_STICK_EPSILON: f32 = 0.02;

/// 画面が表示しうる接続状態（モジュール doc の 3 分岐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamepadView {
    /// 実コントローラを読めている。
    Connected,
    /// 実装は対応しているが接続中のコントローラが無い（代替入力が効く）。
    DisconnectedAlt,
    /// このプラットフォームでは未対応（代替入力が効く）。
    UnsupportedAlt,
}

impl Default for GamepadView {
    fn default() -> Self {
        Self::DisconnectedAlt
    }
}

/// 「ゲームコントローラ」画面の状態。
#[derive(Debug, Clone, PartialEq)]
pub struct GamepadShowState {
    /// 現在の接続状態（表示文言の分岐に使う）。
    pub view: GamepadView,
    /// 玉の絵・物理は `tilt_ball` の実装をそのまま持つ（複製しない、モジュール doc 参照）。
    ball: TiltBallState,
    /// 代替入力 X（`-1.0..=1.0`）。`view` が `Connected` のときは効果を持たない
    /// （選択は effect ドライバの責務、モジュール doc 参照）。
    pub alt_x: f32,
    /// 代替入力 Y（`-1.0..=1.0`）。[`alt_x`](Self::alt_x) と同じ扱い。
    pub alt_y: f32,
    /// 代替入力ボタン（チェックボックス）。
    pub alt_button: bool,
    /// 押下中ボタン名の表示行（effect ドライバが `Gamepad` 側の
    /// `pressed_labels`/`ALT_BUTTON_LABEL` から整形して渡す）。
    pressed: String,
}

impl Default for GamepadShowState {
    fn default() -> Self {
        Self {
            view: GamepadView::default(),
            ball: TiltBallState::default(),
            alt_x: 0.0,
            alt_y: 0.0,
            alt_button: false,
            pressed: String::new(),
        }
    }
}

/// 非有限値（NaN/inf）を `0.0` へ、範囲外の値を `-1.0..=1.0` へ丸める
/// （`tilt_ball::normalize_tilt` と同じ NaN 対策の順序、信頼境界の入力検証）。
fn normalize_alt(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    }
}

/// `Screen::slider` の getter（代替入力 X）。
pub fn get_alt_x(state: &GamepadShowState) -> f32 {
    state.alt_x
}

/// `Screen::slider` の setter（代替入力 X）。非有限値・範囲外値を丸める
/// （`tilt_ball::set_tilt_x` と同じ防御）。
pub fn set_alt_x(state: &mut GamepadShowState, value: f32) {
    state.alt_x = normalize_alt(value);
}

/// `Screen::slider` の getter（代替入力 Y）。
pub fn get_alt_y(state: &GamepadShowState) -> f32 {
    state.alt_y
}

/// `Screen::slider` の setter（代替入力 Y）。[`set_alt_x`] と同じ防御。
pub fn set_alt_y(state: &mut GamepadShowState, value: f32) {
    state.alt_y = normalize_alt(value);
}

/// `Screen::checkbox` の getter（代替入力ボタン）。
pub fn get_alt_button(state: &GamepadShowState) -> bool {
    state.alt_button
}

/// `Screen::checkbox` の setter（代替入力ボタン）。
pub fn set_alt_button(state: &mut GamepadShowState, value: bool) {
    state.alt_button = value;
}

/// 実コントローラを読めているときの 1 フレーム分の更新。`x`/`y` は effect ドライバが
/// 実装の軸の向き契約（画面座標系・y 下が正）へ正規化済みの値を渡す契約
/// （`fandhe_device::gamepad` モジュール doc 参照）。`pressed` は `fandhe_device::gamepad::
/// pressed_labels` の結果を整形した表示用文字列（空文字列は「押下なし」）。
pub fn apply_connected(state: &mut GamepadShowState, x: f32, y: f32, pressed: String, dt: f32) {
    state.view = GamepadView::Connected;
    state.pressed = pressed;
    tilt_ball::step(&mut state.ball, (x, y), dt);
}

/// 実装は対応しているが未接続のときの 1 フレーム分の更新。代替入力（スライダー・
/// チェックボックス）を tilt 入力として使う。
pub fn apply_disconnected(state: &mut GamepadShowState, dt: f32) {
    state.view = GamepadView::DisconnectedAlt;
    apply_alt_frame(state, dt);
}

/// 非対応プラットフォームのときの 1 フレーム分の更新。[`apply_disconnected`] と同じく
/// 代替入力を使うが、表示文言は「非対応」で区別する。
pub fn apply_unsupported(state: &mut GamepadShowState, dt: f32) {
    state.view = GamepadView::UnsupportedAlt;
    apply_alt_frame(state, dt);
}

/// 代替入力（スライダー・チェックボックス）を tilt 入力として玉を進める共通処理
/// （[`apply_disconnected`] / [`apply_unsupported`] から呼ぶ）。
fn apply_alt_frame(state: &mut GamepadShowState, dt: f32) {
    let deadzone = |v: f32| if v.abs() <= ALT_STICK_EPSILON { 0.0 } else { v };
    let (x, y) = (deadzone(state.alt_x), deadzone(state.alt_y));
    state.pressed = if state.alt_button {
        ALT_BUTTON_LABEL.to_string()
    } else {
        String::new()
    };
    tilt_ball::step(&mut state.ball, (x, y), dt);
}

/// 玉が静止中か（`fandhe-egui-proxy::device::gamepad` が「傾き 0 かつ静止中は完了送信を
/// スキップする」判定に使う、`device/tilt.rs::can_skip_send` と同じ考え方）。
pub fn is_at_rest(state: &GamepadShowState) -> bool {
    tilt_ball::is_at_rest(&state.ball)
}

/// 現在表示中の押下中ボタン名（[`apply_connected`] 等の直近呼び出しで書き込んだ値）。
/// effect ドライバの skip-send 判定が「前回と同じ押下状態か」を比較するために使う。
pub fn pressed_text(state: &GamepadShowState) -> &str {
    &state.pressed
}

/// 案内文言 + 玉の位置 + 押下中ボタン名（`DynamicText` の `get`、表示文言の SSOT）。
pub fn status_text(state: &GamepadShowState) -> String {
    let label = match state.view {
        GamepadView::Connected => CONNECTED_LABEL,
        GamepadView::DisconnectedAlt => DISCONNECTED_ALT_LABEL,
        GamepadView::UnsupportedAlt => UNSUPPORTED_ALT_LABEL,
    };
    let (x, y) = state.ball.ball_position();
    let pressed = if state.pressed.is_empty() {
        NO_BUTTON_TEXT
    } else {
        state.pressed.as_str()
    };
    format!("{label} / 玉の位置 ({x:.0}, {y:.0}) / 押下中: {pressed}")
}

/// キャンバスの `paint`（`CanvasWidget::bound` へ渡す）。玉の絵は `tilt_ball` の実装を
/// そのまま呼ぶだけ（モジュール doc 参照）。
fn ball_commands(state: &GamepadShowState) -> Vec<PaintCommand> {
    tilt_ball::ball_commands(&state.ball)
}

/// 「ゲームコントローラ」画面。固定 1 画面（`effect` 区画から `current_screen` 経由で
/// 呼ばれる、モジュール冒頭参照）。
pub fn gamepad_show_screen() -> Screen<GamepadShowState> {
    let canvas_size = Measurement {
        width: tilt_ball::FIELD_SIZE,
        height: tilt_ball::FIELD_SIZE,
    };
    Screen::new("ゲームコントローラ")
        .with_node(Node::Widget(Box::new(CanvasWidget::bound(
            CANVAS_ID,
            "左スティックで玉が動く盤面",
            canvas_size,
            ball_commands,
        ))))
        .dynamic_text(status_text)
        .slider(
            ALT_X_SLIDER_ID,
            "代替入力 X",
            -1.0,
            1.0,
            get_alt_x,
            set_alt_x,
        )
        .slider(
            ALT_Y_SLIDER_ID,
            "代替入力 Y",
            -1.0,
            1.0,
            get_alt_y,
            set_alt_y,
        )
        .checkbox(
            ALT_BUTTON_CHECKBOX_ID,
            "代替ボタン",
            get_alt_button,
            set_alt_button,
        )
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口
/// （`tilt_ball::current_screen` と同型）。画面は固定 1 枚のため中身は
/// [`gamepad_show_screen`] を返すだけでよい。
pub fn current_screen(_state: &GamepadShowState) -> Screen<GamepadShowState> {
    gamepad_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::WidgetValue;
    use crate::{render_headless, set_widget_value_at};

    #[test]
    fn default_state_is_disconnected_alt_with_no_button() {
        let state = GamepadShowState::default();
        assert_eq!(state.view, GamepadView::DisconnectedAlt);
        let text = status_text(&state);
        assert!(text.contains(DISCONNECTED_ALT_LABEL));
        assert!(text.contains(NO_BUTTON_TEXT));
        // Issue #859 の再発防止: 未接続・非対応時に「接続中」を出さない。
        assert!(!text.contains(CONNECTED_LABEL));
    }

    #[test]
    fn connected_disconnected_unsupported_have_distinct_labels() {
        let mut connected = GamepadShowState::default();
        apply_connected(&mut connected, 0.0, 0.0, String::new(), 1.0 / 60.0);
        let mut disconnected = GamepadShowState::default();
        apply_disconnected(&mut disconnected, 1.0 / 60.0);
        let mut unsupported = GamepadShowState::default();
        apply_unsupported(&mut unsupported, 1.0 / 60.0);

        let texts = [
            status_text(&connected),
            status_text(&disconnected),
            status_text(&unsupported),
        ];
        for (i, a) in texts.iter().enumerate() {
            for (j, b) in texts.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "3 状態は互いに異なる文言を持つ");
                }
            }
        }
        assert!(texts[0].contains(CONNECTED_LABEL));
        assert!(texts[1].contains(DISCONNECTED_ALT_LABEL));
        assert!(texts[2].contains(UNSUPPORTED_ALT_LABEL));
    }

    #[test]
    fn apply_connected_moves_ball_with_real_stick_input() {
        let mut state = GamepadShowState::default();
        let start = state.ball.ball_position();
        for _ in 0..10 {
            apply_connected(&mut state, 1.0, 0.0, String::new(), 1.0 / 60.0);
        }
        assert!(state.ball.ball_position().0 > start.0);
    }

    #[test]
    fn apply_disconnected_uses_alt_input_to_move_ball() {
        let mut state = GamepadShowState::default();
        set_alt_x(&mut state, 1.0);
        let start = state.ball.ball_position();
        for _ in 0..10 {
            apply_disconnected(&mut state, 1.0 / 60.0);
        }
        assert!(state.ball.ball_position().0 > start.0);
    }

    #[test]
    fn apply_disconnected_ignores_residual_alt_input_within_deadzone() {
        // ALT_STICK_EPSILON 以下の残留スライダー値は玉に力を与えず、静止させる
        // （Cursor Bugbot 指摘 Medium、PR #937: 畳まないと skip-send 判定が
        // 永久に成立しない）。
        let mut state = GamepadShowState::default();
        set_alt_x(&mut state, ALT_STICK_EPSILON);
        set_alt_y(&mut state, -ALT_STICK_EPSILON);
        for _ in 0..10 {
            apply_disconnected(&mut state, 1.0 / 60.0);
        }
        assert!(is_at_rest(&state));
    }

    #[test]
    fn apply_alt_frame_reports_alt_button_press() {
        let mut state = GamepadShowState::default();
        set_alt_button(&mut state, true);
        apply_disconnected(&mut state, 1.0 / 60.0);
        assert!(status_text(&state).contains("代替ボタン"));

        set_alt_button(&mut state, false);
        apply_disconnected(&mut state, 1.0 / 60.0);
        assert!(status_text(&state).contains(NO_BUTTON_TEXT));
    }

    #[test]
    fn apply_connected_does_not_touch_alt_inputs() {
        // 実コントローラが読めているときは代替入力に触れない（上書きしない契約は
        // effect ドライバ側の選択ロジックが担うが、画面側の状態も壊れないことを固定）。
        let mut state = GamepadShowState::default();
        set_alt_x(&mut state, 0.5);
        set_alt_y(&mut state, -0.5);
        apply_connected(&mut state, 1.0, 1.0, "A".to_string(), 1.0 / 60.0);
        assert_eq!(state.alt_x, 0.5);
        assert_eq!(state.alt_y, -0.5);
    }

    #[test]
    fn set_alt_ignores_non_finite_and_clamps_range() {
        let mut state = GamepadShowState::default();
        set_alt_x(&mut state, 2.0);
        assert_eq!(state.alt_x, 1.0);
        set_alt_x(&mut state, f32::NAN);
        assert_eq!(state.alt_x, 0.0);
        set_alt_y(&mut state, -2.0);
        assert_eq!(state.alt_y, -1.0);
    }

    #[test]
    fn status_text_appears_in_headless_render() {
        let state = GamepadShowState::default();
        let screen = gamepad_show_screen();
        let rendered = render_headless(&screen, &state);
        assert!(
            rendered
                .iter()
                .any(|line| line.contains(DISCONNECTED_ALT_LABEL))
        );
    }

    #[test]
    fn slider_writes_alt_x_through_set_widget_value_at() {
        let screen = gamepad_show_screen();
        let mut state = GamepadShowState::default();
        // ノード順: 0=Canvas, 1=DynamicText, 2=代替入力 X, 3=代替入力 Y, 4=代替ボタン。
        let handled = set_widget_value_at(
            &screen,
            &[2],
            ALT_X_SLIDER_ID,
            &WidgetValue::Number(0.5),
            &mut state,
        );
        assert!(handled);
        assert_eq!(state.alt_x, 0.5);
    }

    #[test]
    fn checkbox_writes_alt_button_through_set_widget_value_at() {
        let screen = gamepad_show_screen();
        let mut state = GamepadShowState::default();
        let handled = set_widget_value_at(
            &screen,
            &[4],
            ALT_BUTTON_CHECKBOX_ID,
            &WidgetValue::Bool(true),
            &mut state,
        );
        assert!(handled);
        assert!(state.alt_button);
    }

    #[test]
    fn gamepad_show_screen_is_registered_as_expected_title() {
        assert_eq!(gamepad_show_screen().title(), "ゲームコントローラ");
    }
}
