//! Phase 11 の拡充ループ型サンプル「ペン・筆圧で描く」（親 #95、Issue #355）。
//! キャンバスをなぞると線が描かれ、ペンの筆圧で太さが変わる。ポインタの種類・筆圧・傾きを
//! 3 行で表示する。`FANDHE_PROXY_APP=pen_draw` で選ばれ、`fandhe-egui-proxy` の
//! `screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod pen_draw;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。位置は
//! `device/pen_stylus.rs` がキャンバス上のポインタから、種類・筆圧・傾きは OS 層から取り、
//! [`apply_sample`] / [`end_stroke`] / [`apply_failure`] で注入する。全 OS 分の状態・定数・
//! 文言をここで定義する（OS 固有の登録・解除 UI は要らず、操作はクリアボタンだけ）。
//!
//! 嘘の成功を出さない: 筆圧はペンが報告したときだけ太さに使い、マウス・指は筆圧の値に
//! 関わらず一定の太さにする。非対応・失敗の状態では描画を受け付けない。古い世代
//! （[`PenDrawState::generation`]、クリアで進む）の完了は捨てる。

use std::collections::VecDeque;

use crate::token::ColorValue;
use crate::ui::CanvasWidget;
use crate::{Measurement, Node, PaintCommand, Screen, WidgetId};

/// キャンバス。`fandhe-egui-proxy` がポインタ位置を読むときの識別子でもあるため `pub`。
pub const CANVAS_ID: WidgetId = WidgetId::from_raw(1);
/// クリアボタン。
pub const CLEAR_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// クリアの文言。
pub const CLEAR_LABEL: &str = "クリア";
/// キャンバスの一辺（論理ピクセル相当）。
pub const CANVAS_SIZE: f32 = 320.0;
/// 保持する点の上限。
// ponytail: 古い点から捨てる。Web では DOM の div 数が天井。必要なら線分描画に替える。
pub const MAX_POINTS: usize = 2000;
/// 筆圧が無いときの太さ（半径）。
pub const BASE_RADIUS: f32 = 3.0;
/// 筆圧 0 の太さ。
pub const MIN_RADIUS: f32 = 1.0;
/// 筆圧 1 の太さ。
pub const MAX_RADIUS: f32 = 8.0;
/// 補間点の間隔。
pub const POINT_SPACING: f32 = 2.0;
/// 1 区間あたりの補間点数の上限（異常な座標で処理が膨らまないようにする）。
pub const MAX_STEPS: usize = 64;

/// 失敗時の案内（非対応）。
pub const UNSUPPORTED_TEXT: &str = "この OS ではペン入力を読めません";
/// 失敗時の案内（その他）。
pub const FAILED_TEXT: &str = "ペン入力を読めませんでした";
/// 待機中の案内。
pub const IDLE_TEXT: &str = "キャンバスをなぞってください";
/// 入力中の案内。
pub const LIVE_TEXT: &str = "入力中";

/// ポインタの種類（画面側の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKindView {
    /// ペン。
    Pen,
    /// 指。
    Touch,
    /// マウス。
    Mouse,
    /// 不明。
    Unknown,
}

/// 注入する 1 サンプル。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PenSampleView {
    /// 種類。
    pub kind: PointerKindView,
    /// 筆圧（`0..=1`）。無ければ `None`。
    pub pressure: Option<f32>,
    /// 高度角（度）。無ければ `None`。
    pub altitude_deg: Option<f32>,
}

/// 失敗の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenFailureView {
    /// この OS では読めない。
    Unsupported,
    /// 読み取りの失敗。
    Failed,
}

/// 画面の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenStatus {
    /// 入力なし。
    Idle,
    /// 描画中。
    Live,
    /// 失敗（以後の描画を受け付けない）。
    Failure(PenFailureView),
}

/// 「ペン・筆圧で描く」画面の状態。
#[derive(Debug, Clone, PartialEq)]
pub struct PenDrawState {
    /// 表示中の種類。接触なしは `None`。
    pub kind: Option<PointerKindView>,
    /// 表示中の筆圧。
    pub pressure: Option<f32>,
    /// 表示中の高度角（度）。
    pub altitude: Option<f32>,
    /// 状態。
    pub status: PenStatus,
    /// 世代。クリアで進み、古い世代の完了は捨てる。
    pub generation: u64,
    points: VecDeque<(f32, f32, f32)>,
    last_point: Option<(f32, f32)>,
}

impl Default for PenDrawState {
    fn default() -> Self {
        Self {
            kind: None,
            pressure: None,
            altitude: None,
            status: PenStatus::Idle,
            generation: 0,
            points: VecDeque::new(),
            last_point: None,
        }
    }
}

impl PenDrawState {
    /// 保持している点の数。
    pub fn point_count(&self) -> usize {
        self.points.len()
    }

    /// 進行中のストロークがあるか（表示値か直前の点がある）。
    pub fn is_stroking(&self) -> bool {
        self.last_point.is_some() || self.kind.is_some()
    }
}

/// 種類と筆圧から太さ（半径）を決める。ペンかつ筆圧ありのときだけ可変（嘘の成功の禁止）。
pub fn radius_for(kind: PointerKindView, pressure: Option<f32>) -> f32 {
    match (kind, pressure) {
        (PointerKindView::Pen, Some(p)) if p.is_finite() => {
            MIN_RADIUS + (MAX_RADIUS - MIN_RADIUS) * p.clamp(0.0, 1.0)
        }
        _ => BASE_RADIUS,
    }
}

fn in_canvas(x: f32, y: f32) -> bool {
    x.is_finite()
        && y.is_finite()
        && (0.0..=CANVAS_SIZE).contains(&x)
        && (0.0..=CANVAS_SIZE).contains(&y)
}

fn push_point(s: &mut PenDrawState, x: f32, y: f32, r: f32) {
    if s.points.len() >= MAX_POINTS {
        s.points.pop_front();
    }
    s.points.push_back((x, y, r));
}

/// キャンバス上の位置 `pos` と OS のサンプルを注入する。`sample` が `None`（OS が値を返さない）
/// のときは種類不明・一定の太さで描く。世代の不一致・失敗状態・範囲外の座標は捨てる（範囲外は補間の起点も解除する）。
pub fn apply_sample(
    s: &mut PenDrawState,
    generation: u64,
    pos: (f32, f32),
    sample: Option<PenSampleView>,
) {
    if generation != s.generation || matches!(s.status, PenStatus::Failure(_)) {
        return;
    }
    let (x, y) = pos;
    if !in_canvas(x, y) {
        // 範囲外では補間の起点を解除する。再入場時に退出前の点と直線で結ばない。
        s.last_point = None;
        return;
    }
    let (kind, pressure, altitude) = match sample {
        Some(v) => (v.kind, v.pressure, v.altitude_deg),
        None => (PointerKindView::Unknown, None, None),
    };
    let r = radius_for(kind, pressure);
    match s.last_point {
        Some((lx, ly)) => {
            let d = ((x - lx).powi(2) + (y - ly).powi(2)).sqrt();
            let steps = ((d / POINT_SPACING).ceil() as usize).clamp(1, MAX_STEPS);
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                push_point(s, lx + (x - lx) * t, ly + (y - ly) * t, r);
            }
        }
        None => push_point(s, x, y, r),
    }
    s.last_point = Some((x, y));
    s.kind = Some(kind);
    s.pressure = pressure;
    s.altitude = altitude;
    s.status = PenStatus::Live;
}

/// 離した・キャンセル・背景遷移で呼ぶ。表示を「なし」に戻し、線は残す。古い世代は捨てる。
pub fn end_stroke(s: &mut PenDrawState, generation: u64) {
    if generation != s.generation {
        return;
    }
    s.last_point = None;
    s.kind = None;
    s.pressure = None;
    s.altitude = None;
    if s.status == PenStatus::Live {
        s.status = PenStatus::Idle;
    }
}

/// 失敗を確定する。以後 [`apply_sample`] は無視される。
pub fn apply_failure(s: &mut PenDrawState, f: PenFailureView) {
    s.status = PenStatus::Failure(f);
    s.last_point = None;
    s.kind = None;
    s.pressure = None;
    s.altitude = None;
}

/// クリアボタン。線を消して世代を進める（クリア前に積まれた完了を捨てる）。失敗状態は維持する。
pub fn clear(s: &mut PenDrawState) {
    s.generation += 1;
    s.points.clear();
    s.last_point = None;
    s.kind = None;
    s.pressure = None;
    s.altitude = None;
    if s.status == PenStatus::Live {
        s.status = PenStatus::Idle;
    }
}

/// 種類の 1 行。
pub fn kind_text(s: &PenDrawState) -> String {
    let t = match s.kind {
        Some(PointerKindView::Pen) => "ペン",
        Some(PointerKindView::Touch) => "指",
        Some(PointerKindView::Mouse) => "マウス",
        Some(PointerKindView::Unknown) => "不明",
        None => "なし",
    };
    format!("種類: {t}")
}

/// 筆圧の 1 行。
pub fn pressure_text(s: &PenDrawState) -> String {
    match s.pressure {
        Some(p) => format!("筆圧: {p:.2}"),
        None => "筆圧: なし".to_string(),
    }
}

/// 傾きの 1 行。
pub fn altitude_text(s: &PenDrawState) -> String {
    match s.altitude {
        Some(a) => format!("傾き: {a:.0}°"),
        None => "傾き: なし".to_string(),
    }
}

/// 状態の 1 行。
pub fn status_text(s: &PenDrawState) -> String {
    match s.status {
        PenStatus::Idle => IDLE_TEXT,
        PenStatus::Live => LIVE_TEXT,
        PenStatus::Failure(PenFailureView::Unsupported) => UNSUPPORTED_TEXT,
        PenStatus::Failure(PenFailureView::Failed) => FAILED_TEXT,
    }
    .to_string()
}

/// 背景 + 点の描画コマンド列（キャンバス左上原点）。
pub fn paint(s: &PenDrawState) -> Vec<PaintCommand> {
    let mut v = Vec::with_capacity(s.points.len() + 1);
    v.push(PaintCommand::StyledRect {
        fill: ColorValue::rgb(245, 245, 248),
    });
    for &(x, y, r) in &s.points {
        v.push(PaintCommand::Circle {
            center: (x, y),
            radius: r,
            fill: ColorValue::rgb(30, 30, 40),
        });
    }
    v
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。固定 1 画面。
pub fn current_screen(_s: &PenDrawState) -> Screen<PenDrawState> {
    Screen::new("ペン・筆圧")
        .dynamic_text(kind_text)
        .dynamic_text(pressure_text)
        .dynamic_text(altitude_text)
        .dynamic_text(status_text)
        .with_node(Node::Widget(Box::new(
            CanvasWidget::bound(
                CANVAS_ID,
                "描画キャンバス",
                Measurement {
                    width: CANVAS_SIZE,
                    height: CANVAS_SIZE,
                },
                paint,
            )
            .with_drag_capture(),
        )))
        .button(CLEAR_BUTTON_ID, CLEAR_LABEL, clear)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pen(p: f32) -> Option<PenSampleView> {
        Some(PenSampleView {
            kind: PointerKindView::Pen,
            pressure: Some(p),
            altitude_deg: Some(45.0),
        })
    }

    #[test]
    fn leaving_canvas_breaks_the_line() {
        let mut s = PenDrawState::default();
        apply_sample(&mut s, 0, (10.0, 10.0), pen(0.5));
        apply_sample(&mut s, 0, (-10.0, 10.0), pen(0.5));
        apply_sample(&mut s, 0, (-10.0, 300.0), pen(0.5));
        apply_sample(&mut s, 0, (10.0, 300.0), pen(0.5));
        // 再入場の点だけが加わり、縦線の補間点は出ない。
        assert_eq!(s.point_count(), 2);
    }

    #[test]
    fn adds_and_interpolates_points() {
        let mut s = PenDrawState::default();
        apply_sample(&mut s, 0, (10.0, 10.0), pen(0.5));
        assert_eq!(s.point_count(), 1);
        apply_sample(&mut s, 0, (30.0, 10.0), pen(0.5));
        assert_eq!(s.point_count(), 1 + 10);
        assert_eq!(s.status, PenStatus::Live);
        assert_eq!(kind_text(&s), "種類: ペン");
        assert_eq!(pressure_text(&s), "筆圧: 0.50");
        assert_eq!(altitude_text(&s), "傾き: 45°");
    }

    #[test]
    fn pen_pressure_changes_radius_but_others_stay_constant() {
        assert!(
            radius_for(PointerKindView::Pen, Some(1.0))
                > radius_for(PointerKindView::Pen, Some(0.1))
        );
        for k in [
            PointerKindView::Mouse,
            PointerKindView::Touch,
            PointerKindView::Unknown,
        ] {
            assert_eq!(radius_for(k, Some(0.5)), BASE_RADIUS);
            assert_eq!(radius_for(k, Some(1.0)), BASE_RADIUS);
        }
        assert_eq!(radius_for(PointerKindView::Pen, None), BASE_RADIUS);
        assert_eq!(
            radius_for(PointerKindView::Pen, Some(f32::NAN)),
            BASE_RADIUS
        );
    }

    #[test]
    fn none_sample_draws_unknown_constant() {
        let mut s = PenDrawState::default();
        apply_sample(&mut s, 0, (5.0, 5.0), None);
        assert_eq!(s.kind, Some(PointerKindView::Unknown));
        assert_eq!(pressure_text(&s), "筆圧: なし");
    }

    #[test]
    fn stale_generation_is_dropped_after_clear() {
        let mut s = PenDrawState::default();
        apply_sample(&mut s, 0, (5.0, 5.0), pen(0.5));
        clear(&mut s);
        assert_eq!(s.point_count(), 0);
        apply_sample(&mut s, 0, (6.0, 6.0), pen(0.5));
        assert_eq!(s.point_count(), 0);
        assert_eq!(s.status, PenStatus::Idle);
        s.kind = Some(PointerKindView::Pen);
        end_stroke(&mut s, 0);
        assert_eq!(s.kind, Some(PointerKindView::Pen));
        apply_sample(&mut s, 1, (6.0, 6.0), pen(0.5));
        assert_eq!(s.point_count(), 1);
    }

    #[test]
    fn end_stroke_resets_display_and_keeps_line() {
        let mut s = PenDrawState::default();
        apply_sample(&mut s, 0, (5.0, 5.0), pen(0.5));
        end_stroke(&mut s, 0);
        assert_eq!(kind_text(&s), "種類: なし");
        assert_eq!(pressure_text(&s), "筆圧: なし");
        assert_eq!(altitude_text(&s), "傾き: なし");
        assert_eq!(s.point_count(), 1);
        assert!(!s.is_stroking());
        // 離した後の次の接触は補間せず新しい点から始まる。
        apply_sample(&mut s, 0, (100.0, 100.0), pen(0.5));
        assert_eq!(s.point_count(), 2);
    }

    #[test]
    fn caps_points_dropping_oldest() {
        let mut s = PenDrawState::default();
        for i in 0..(MAX_POINTS + 50) {
            end_stroke(&mut s, 0);
            apply_sample(&mut s, 0, ((i % 300) as f32, 1.0), pen(0.5));
        }
        assert_eq!(s.point_count(), MAX_POINTS);
    }

    #[test]
    fn rejects_non_finite_and_out_of_canvas() {
        let mut s = PenDrawState::default();
        for p in [
            (f32::NAN, 1.0),
            (1.0, f32::INFINITY),
            (-1.0, 1.0),
            (1.0, CANVAS_SIZE + 1.0),
        ] {
            apply_sample(&mut s, 0, p, pen(0.5));
        }
        assert_eq!(s.point_count(), 0);
        assert_eq!(s.status, PenStatus::Idle);
    }

    #[test]
    fn huge_jump_is_bounded() {
        let mut s = PenDrawState::default();
        apply_sample(&mut s, 0, (0.0, 0.0), pen(0.5));
        apply_sample(&mut s, 0, (CANVAS_SIZE, CANVAS_SIZE), pen(0.5));
        assert!(s.point_count() <= 1 + MAX_STEPS);
    }

    #[test]
    fn failure_blocks_drawing_and_survives_clear() {
        let mut s = PenDrawState::default();
        apply_failure(&mut s, PenFailureView::Unsupported);
        apply_sample(&mut s, 0, (5.0, 5.0), pen(0.5));
        assert_eq!(s.point_count(), 0);
        assert_eq!(status_text(&s), UNSUPPORTED_TEXT);
        clear(&mut s);
        assert_eq!(s.status, PenStatus::Failure(PenFailureView::Unsupported));
        apply_failure(&mut s, PenFailureView::Failed);
        assert_eq!(status_text(&s), FAILED_TEXT);
    }

    #[test]
    fn paint_has_background_and_circles() {
        let mut s = PenDrawState::default();
        apply_sample(&mut s, 0, (5.0, 5.0), pen(0.5));
        let cmds = paint(&s);
        assert_eq!(cmds.len(), 2);
        assert!(matches!(cmds[0], PaintCommand::StyledRect { .. }));
    }

    #[test]
    fn screen_builds() {
        let s = PenDrawState::default();
        let _ = current_screen(&s);
        assert_eq!(kind_text(&s), "種類: なし");
        assert_eq!(status_text(&s), IDLE_TEXT);
    }
}
