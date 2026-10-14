//! Phase 10（親 #827）型サンプル「傾きで玉が転がる」（Issue #830）。
//! `FANDHE_PROXY_APP=tilt_ball` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼び出される（`screens/mod.rs` の `pub mod tilt_ball;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。物理更新（[`step`]）は純関数で
//! テスト可能にし、傾きセンサ値の取得・供給は `fandhe-egui-proxy::device::tilt`
//! （Mac は `DummyMotion` へスライダーが書き込む、実機は #831/#832 が push する）が
//! 毎フレーム [`step`] を呼び出す effect ドライバとして担う。
//!
//! # 座標系（#831/#832 が実機の軸を合わせる基準）
//!
//! キャンバス左上原点、x は右方向・y は下方向が正。`tilt.0 > 0.0` で玉は右へ、
//! `tilt.1 > 0.0` で玉は下へ加速する。玉は盤面の縁（`[BALL_RADIUS, FIELD_SIZE -
//! BALL_RADIUS]`）を超えず、当たった軸の速度は 0 になる（跳ね返らず縁で止まる）。
//!
//! `tilt_x` / `tilt_y`（`TiltBallState` の公開フィールド）はデスクトップでは
//! スライダーの入力値、実機では観測したセンサ値の表示器として同じフィールドを兼ねる
//! （`fandhe-egui-proxy::device::tilt` が完了クロージャで書き戻す契約）。

use crate::ui::CanvasWidget;
use crate::{Measurement, Node, PaintCommand, Screen, WidgetId};

/// 盤面の一辺（論理ピクセル相当）。
pub const FIELD_SIZE: f32 = 320.0;
/// 玉の半径（論理ピクセル相当）。
pub const BALL_RADIUS: f32 = 14.0;
/// 傾き最大値（`-1.0..=1.0`）でのフレーム秒あたり加速度。
const ACCEL: f32 = 600.0;
/// 減衰係数（1 秒あたりの速度保持率が `(1.0 - DAMPING * dt)`）。
const DAMPING: f32 = 1.5;
/// 1 フレームで進める時間の上限（ProMotion 120Hz・バックグラウンド復帰の飛び対策）。
const MAX_DT: f32 = 0.1;
/// [`is_at_rest`] が「静止」とみなす速度の閾値（論理ピクセル/秒）。
const REST_EPSILON: f32 = 0.5;

const CANVAS_ID: WidgetId = WidgetId::from_raw(1);
const TILT_X_SLIDER_ID: WidgetId = WidgetId::from_raw(2);
const TILT_Y_SLIDER_ID: WidgetId = WidgetId::from_raw(3);

/// センサ取得状況（Issue #989）。egui 経路（macOS/Windows のスライダー・iOS/Android の
/// 実センサ）は常に値を持てるため既定 `Live` のまま挙動が変わらない。Web はセンサ無し・
/// 許可拒否・無応答を `Unavailable` へ、取得試行中を `Waiting` へ明示することで、
/// 0 の傾きを成功値として出さない（device-bridge-contracts.md 6「嘘の成功」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SensorStatus {
    /// 値を継続取得できている（既定。egui 経路はこのまま変わらない）。
    #[default]
    Live,
    /// 取得を試みている（初回応答待ち）。
    Waiting,
    /// 取得できない（センサが無い・許可されていない・無応答）。
    Unavailable,
}

/// 「傾きで玉が転がる」画面が保持する状態。
#[derive(Debug, Clone, PartialEq)]
pub struct TiltBallState {
    /// 傾き X（`-1.0..=1.0`）。デスクトップではスライダー入力値、実機では観測値の表示器。
    pub tilt_x: f32,
    /// 傾き Y（`-1.0..=1.0`）。`tilt_x` と同じ二重の役割を持つ。
    pub tilt_y: f32,
    /// センサ取得状況（[`SensorStatus`] 参照）。[`status_text`] の表示分岐に使う。
    pub sensor: SensorStatus,
    /// 玉の位置（キャンバス左上原点のローカル座標）。[`step`] のみが更新する。
    ball: (f32, f32),
    /// 玉の速度（論理ピクセル/秒）。[`step`] のみが更新する。
    velocity: (f32, f32),
}

impl Default for TiltBallState {
    /// 玉を盤面中央に静止した状態で初期化する。
    fn default() -> Self {
        Self {
            tilt_x: 0.0,
            tilt_y: 0.0,
            sensor: SensorStatus::Live,
            ball: (FIELD_SIZE / 2.0, FIELD_SIZE / 2.0),
            velocity: (0.0, 0.0),
        }
    }
}

impl TiltBallState {
    /// 玉の現在位置（キャンバス左上原点のローカル座標）。
    pub fn ball_position(&self) -> (f32, f32) {
        self.ball
    }

    /// 玉の現在速度（論理ピクセル/秒）。
    pub fn velocity(&self) -> (f32, f32) {
        self.velocity
    }
}

/// 非有限値（NaN/inf）を `0.0` へ、範囲外の値を `-1.0..=1.0` へ丸める
/// （`f32::clamp` は NaN をそのまま通すため先に有限値化する、`DummyMotion::set` と
/// 同じ順序。信頼境界の入力検証、security.md A04）。
fn normalize_tilt(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    }
}

/// 傾き `tilt` と経過時間 `dt`（秒）から玉の物理状態を 1 ステップ進める純関数。
///
/// `dt` が非有限・0 以下の場合は何もしない（フレーム間隔の異常値で状態が壊れない
/// ようにするガード）。`dt` は [`MAX_DT`] でクランプし、`tilt` は [`normalize_tilt`]
/// で NaN 除去・範囲クランプしてから使う。速度 → 減衰 → 位置の順に更新し、盤面の縁で
/// 位置をクランプ・当該軸の速度を 0 にする（跳ね返らず「縁で止まる」、モジュール doc
/// の座標系参照）。
pub fn step(state: &mut TiltBallState, tilt: (f32, f32), dt: f32) {
    if !dt.is_finite() || dt <= 0.0 {
        return;
    }
    let dt = dt.min(MAX_DT);
    let tx = normalize_tilt(tilt.0);
    let ty = normalize_tilt(tilt.1);

    let damping = (1.0 - DAMPING * dt).clamp(0.0, 1.0);
    let mut vx = (state.velocity.0 + tx * ACCEL * dt) * damping;
    let mut vy = (state.velocity.1 + ty * ACCEL * dt) * damping;

    let mut px = state.ball.0 + vx * dt;
    let mut py = state.ball.1 + vy * dt;

    let min = BALL_RADIUS;
    let max = FIELD_SIZE - BALL_RADIUS;
    if px < min {
        px = min;
        vx = 0.0;
    } else if px > max {
        px = max;
        vx = 0.0;
    }
    if py < min {
        py = min;
        vy = 0.0;
    } else if py > max {
        py = max;
        vy = 0.0;
    }

    state.ball = (px, py);
    state.velocity = (vx, vy);
}

/// 玉がほぼ静止しているか（速度成分が両方とも [`REST_EPSILON`] 未満）を判定する。
/// `fandhe-egui-proxy::device::tilt` が「傾き 0 かつ静止中はフレームを回し続けない」
/// 判定に使う。
pub fn is_at_rest(state: &TiltBallState) -> bool {
    state.velocity.0.abs() < REST_EPSILON && state.velocity.1.abs() < REST_EPSILON
}

/// `Screen::slider` の getter（傾き X）。
pub fn get_tilt_x(state: &TiltBallState) -> f32 {
    state.tilt_x
}

/// `Screen::slider` の setter（傾き X）。非有限値は現在値を維持する
/// （`settings::set_font_size` と同じ防御、信頼境界の入力検証）。
pub fn set_tilt_x(state: &mut TiltBallState, value: f32) {
    if value.is_finite() {
        state.tilt_x = value.clamp(-1.0, 1.0);
    }
}

/// `Screen::slider` の getter（傾き Y）。
pub fn get_tilt_y(state: &TiltBallState) -> f32 {
    state.tilt_y
}

/// `Screen::slider` の setter（傾き Y）。[`set_tilt_x`] と同じ防御。
pub fn set_tilt_y(state: &mut TiltBallState, value: f32) {
    if value.is_finite() {
        state.tilt_y = value.clamp(-1.0, 1.0);
    }
}

/// 盤面（`StyledRect`）+ 玉（`Circle`）の描画コマンド列（キャンバス左上原点の
/// ローカル座標）。`CanvasWidget::bound` の `paint` 関数として渡す。
pub fn ball_commands(state: &TiltBallState) -> Vec<PaintCommand> {
    let (x, y) = state.ball;
    vec![
        PaintCommand::StyledRect {
            fill: crate::token::ColorValue::rgb(230, 230, 235),
        },
        PaintCommand::Circle {
            center: (x, y),
            radius: BALL_RADIUS,
            fill: crate::token::ColorValue::rgb(220, 60, 60),
        },
    ]
}

/// 傾きと玉の座標を示す 1 行（`render_headless` は `Node::Widget` を出力しないため、
/// ヘッドレス検証と a11y の両方を本関数が担う）。
///
/// [`SensorStatus::Waiting`]/[`SensorStatus::Unavailable`] は数値（0 の傾き）を
/// 一切出さない（Web の初期表示・センサ無しを成功値と見せない、
/// device-bridge-contracts.md 6「嘘の成功」）。
pub fn status_text(state: &TiltBallState) -> String {
    match state.sensor {
        SensorStatus::Live => {
            let (x, y) = state.ball;
            format!(
                "傾き ({:.2}, {:.2}) / 玉の位置 ({:.0}, {:.0})",
                state.tilt_x, state.tilt_y, x, y
            )
        }
        SensorStatus::Waiting => "傾きを取得しています".to_string(),
        SensorStatus::Unavailable => {
            "傾きを取得できない（センサが無い・許可されていない）".to_string()
        }
    }
}

/// 「傾きで玉が転がる」画面。固定 1 画面（`effect` 区画から `current_screen` 経由で
/// 呼ばれる、モジュール冒頭参照）。
pub fn tilt_ball_screen() -> Screen<TiltBallState> {
    let canvas_size = Measurement {
        width: FIELD_SIZE,
        height: FIELD_SIZE,
    };
    Screen::new("傾きで玉が転がる")
        .with_node(Node::Widget(Box::new(CanvasWidget::bound(
            CANVAS_ID,
            "玉が転がる盤面",
            canvas_size,
            ball_commands,
        ))))
        .dynamic_text(status_text)
        .slider(
            TILT_X_SLIDER_ID,
            "傾き X",
            -1.0,
            1.0,
            get_tilt_x,
            set_tilt_x,
        )
        .slider(
            TILT_Y_SLIDER_ID,
            "傾き Y",
            -1.0,
            1.0,
            get_tilt_y,
            set_tilt_y,
        )
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口
/// （`http_fetch::current_screen` と同型）。画面は固定 1 枚のため中身は
/// [`tilt_ball_screen`] を返すだけでよい。
pub fn current_screen(_state: &TiltBallState) -> Screen<TiltBallState> {
    tilt_ball_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::WidgetValue;
    use crate::{render_headless, set_widget_value_at};

    #[test]
    fn right_tilt_increases_ball_x() {
        let mut state = TiltBallState::default();
        let start_x = state.ball_position().0;
        for _ in 0..10 {
            step(&mut state, (1.0, 0.0), 1.0 / 60.0);
        }
        assert!(state.ball_position().0 > start_x);
    }

    #[test]
    fn down_tilt_increases_ball_y() {
        let mut state = TiltBallState::default();
        let start_y = state.ball_position().1;
        for _ in 0..10 {
            step(&mut state, (0.0, 1.0), 1.0 / 60.0);
        }
        assert!(state.ball_position().1 > start_y);
    }

    #[test]
    fn sustained_tilt_stops_at_edge_without_bouncing() {
        let mut state = TiltBallState::default();
        for _ in 0..1000 {
            step(&mut state, (1.0, 0.0), 1.0 / 60.0);
        }
        let (x, _) = state.ball_position();
        assert!((x - (FIELD_SIZE - BALL_RADIUS)).abs() < f32::EPSILON);
        assert_eq!(state.velocity().0, 0.0);
    }

    #[test]
    fn zero_tilt_decays_to_rest() {
        let mut state = TiltBallState::default();
        step(&mut state, (1.0, 0.0), 0.2);
        assert!(!is_at_rest(&state));
        for _ in 0..200 {
            step(&mut state, (0.0, 0.0), 1.0 / 60.0);
        }
        assert!(is_at_rest(&state));
    }

    #[test]
    fn step_ignores_non_positive_or_non_finite_dt() {
        let mut state = TiltBallState::default();
        let before = state.clone();
        step(&mut state, (1.0, 1.0), 0.0);
        assert_eq!(state, before);
        step(&mut state, (1.0, 1.0), -1.0);
        assert_eq!(state, before);
        step(&mut state, (1.0, 1.0), f32::NAN);
        assert_eq!(state, before);
    }

    #[test]
    fn step_normalizes_nan_and_out_of_range_tilt() {
        let mut state = TiltBallState::default();
        // NaN 傾きは 0 扱いになり玉は動かない。
        step(&mut state, (f32::NAN, f32::NAN), 1.0 / 60.0);
        assert_eq!(state, TiltBallState::default());

        // 範囲外の傾きは 1.0 相当にクランプされ、位置・速度は有限のまま。
        let mut state = TiltBallState::default();
        step(&mut state, (10.0, -10.0), 1.0 / 60.0);
        assert!(state.ball_position().0.is_finite());
        assert!(state.ball_position().1.is_finite());
    }

    #[test]
    fn set_tilt_ignores_non_finite_and_clamps_range() {
        let mut state = TiltBallState::default();
        set_tilt_x(&mut state, 2.0);
        assert_eq!(state.tilt_x, 1.0);
        set_tilt_x(&mut state, f32::NAN);
        assert_eq!(state.tilt_x, 1.0);
        set_tilt_y(&mut state, -2.0);
        assert_eq!(state.tilt_y, -1.0);
    }

    #[test]
    fn status_text_appears_in_headless_render() {
        let state = TiltBallState::default();
        let screen = tilt_ball_screen();
        let rendered = render_headless(&screen, &state);
        assert!(rendered.iter().any(|line| line.contains("傾き (")));
    }

    #[test]
    fn slider_writes_tilt_x_through_set_widget_value_at() {
        let screen = tilt_ball_screen();
        let mut state = TiltBallState::default();
        // ノード順: 0=Canvas, 1=DynamicText, 2=傾き X スライダー, 3=傾き Y スライダー。
        let handled = set_widget_value_at(
            &screen,
            &[2],
            TILT_X_SLIDER_ID,
            &WidgetValue::Number(0.5),
            &mut state,
        );
        assert!(handled);
        assert_eq!(state.tilt_x, 0.5);
    }

    #[test]
    fn tilt_ball_screen_is_registered_as_expected_title() {
        assert_eq!(tilt_ball_screen().title(), "傾きで玉が転がる");
    }

    #[test]
    fn status_text_is_unchanged_for_default_live_sensor() {
        let state = TiltBallState::default();
        assert!(status_text(&state).starts_with("傾き ("));
    }

    #[test]
    fn status_text_hides_numbers_while_waiting_or_unavailable() {
        let mut state = TiltBallState {
            sensor: SensorStatus::Waiting,
            ..Default::default()
        };
        let text = status_text(&state);
        assert!(!text.contains("傾き ("));
        assert!(text.contains("取得しています"));

        state.sensor = SensorStatus::Unavailable;
        let text = status_text(&state);
        assert!(!text.contains("傾き ("));
        assert!(text.contains("取得できない"));
    }
}
