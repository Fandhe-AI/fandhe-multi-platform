//! Phase 11 の拡充ループ（親 #1132）型サンプル「アクセシビリティ設定の追従」（Issue #1308）。
//! `FANDHE_PROXY_APP=accessibility_prefs_show` で選ばれ、`fandhe-egui-proxy` の
//! `screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の
//! `pub mod accessibility_prefs_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `fandhe-egui-proxy` の `device/accessibility_prefs.rs`（Web は `fandhe-web-dom`）が一定間隔で
//! 行い、[`apply_read`] で結果を注入する。`fandhe_device::accessibility_prefs` の型は
//! 依存方向の都合でここに写し取る。
//!
//! 視差軽減が「オン」のときだけ丸を止め、コントラスト増加が「オン」のときだけ枠を太くする。
//! 不明（`None`）・取得中・非対応・失敗では丸を動かし枠は通常の太さにする（止めると「オン」と
//! 同じ見え方になり嘘の成功になるため）。失敗時は最後に読めた値を残さない。
//! `PaintCommand` には枠線が無いので、外側に枠色の円、内側に塗りの円を重ねて枠を表す。

use crate::token::ColorValue;
use crate::ui::CanvasWidget;
use crate::{Measurement, Node, PaintCommand, Screen, WidgetId};

/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 盤面の幅（論理ピクセル相当）。
pub const FIELD_W: f32 = 320.0;
/// 盤面の高さ（論理ピクセル相当）。
pub const FIELD_H: f32 = 120.0;
/// 丸の半径。
pub const BALL_RADIUS: f32 = 14.0;
/// 通常時の枠の太さ。
pub const NORMAL_BORDER: f32 = 1.0;
/// コントラスト増加時の枠の太さ。
pub const CONTRAST_BORDER: f32 = 4.0;
/// 丸が片道を往復する 1 周期の秒数。
pub const CYCLE_SECS: f32 = 3.0;

const CANVAS_ID: WidgetId = WidgetId::from_raw(1);

/// OS から読めた値（`fandhe_device::accessibility_prefs::AccessibilityPrefs` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Prefs {
    /// 視差軽減がオンか（`None` は不明）。
    pub reduce_motion: Option<bool>,
    /// コントラスト増加がオンか（`None` は不明）。
    pub increase_contrast: Option<bool>,
}

/// 読めなかった理由。`DeviceError` からの写像は呼び出し元（proxy）の責務。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// この環境では読めない。
    Unsupported,
    /// 内部エラー（固定文言のみ。OS 固有値を持たない）。
    Failed(String),
}

/// 画面に出す設定の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PrefsView {
    /// 最初の値が届く前。
    #[default]
    Reading,
    /// 読めた。
    Read(Prefs),
    /// 非対応。
    Unsupported,
    /// 失敗。
    Failed(String),
}

/// 「アクセシビリティ設定の追従」画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AccessibilityPrefsShowState {
    /// 丸の位相（`0.0..1.0`）。
    pub phase: f32,
    /// 現在の設定の表示状態。
    pub view: PrefsView,
}

/// 読み取り結果を反映する。毎回 view を置き換えるため、失敗後に前の値は残らない。
pub fn apply_read(state: &mut AccessibilityPrefsShowState, result: Result<Prefs, ReadFailure>) {
    state.view = match result {
        Ok(p) => PrefsView::Read(p),
        Err(ReadFailure::Unsupported) => PrefsView::Unsupported,
        Err(ReadFailure::Failed(m)) => PrefsView::Failed(m),
    };
}

fn prefs(state: &AccessibilityPrefsShowState) -> Prefs {
    match &state.view {
        PrefsView::Read(p) => *p,
        _ => Prefs::default(),
    }
}

/// 丸を動かすか。視差軽減が `Some(true)` のときだけ止める（不明・失敗では動かす）。
pub fn animates(state: &AccessibilityPrefsShowState) -> bool {
    prefs(state).reduce_motion != Some(true)
}

/// 位相を `dt` 秒進める（`0.0..1.0` で周回）。
pub fn advance_phase(state: &mut AccessibilityPrefsShowState, dt: f32) {
    state.phase = (state.phase + dt / CYCLE_SECS).rem_euclid(1.0);
}

/// 枠の太さ。コントラスト増加が `Some(true)` のときだけ太くする。
pub fn border_width(state: &AccessibilityPrefsShowState) -> f32 {
    if prefs(state).increase_contrast == Some(true) {
        CONTRAST_BORDER
    } else {
        NORMAL_BORDER
    }
}

/// 盤面の描画コマンド（背景 → 外側の枠色の円 → 内側の塗りの円。ローカル座標）。
/// `CanvasWidget::bound` の `paint` 関数として渡す。
pub fn ball_commands(state: &AccessibilityPrefsShowState) -> Vec<PaintCommand> {
    let w = border_width(state);
    let travel = FIELD_W - 2.0 * (BALL_RADIUS + CONTRAST_BORDER);
    // 三角波で往復させる。
    let t = 1.0 - (2.0 * state.phase - 1.0).abs();
    let x = BALL_RADIUS + CONTRAST_BORDER + travel * t;
    let center = (x, FIELD_H / 2.0);
    vec![
        PaintCommand::StyledRect {
            fill: ColorValue::rgb(230, 230, 235),
        },
        PaintCommand::Circle {
            center,
            radius: BALL_RADIUS + w,
            fill: ColorValue::rgb(20, 20, 20),
        },
        PaintCommand::Circle {
            center,
            radius: BALL_RADIUS,
            fill: ColorValue::rgb(220, 60, 60),
        },
    ]
}

fn onoff(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "オン",
        Some(false) => "オフ",
        None => "不明",
    }
}

/// 2 設定の値を 1 行ずつ出す文言。`render_headless` は Canvas を出力しないため、
/// ヘッドレス検証と a11y の両方を本関数が担う。
pub fn status_text(state: &AccessibilityPrefsShowState) -> String {
    match &state.view {
        PrefsView::Reading => "設定を読み取っています".to_string(),
        PrefsView::Unsupported => "この環境では設定を読めません".to_string(),
        PrefsView::Failed(_) => "設定: 不明".to_string(),
        PrefsView::Read(p) => format!(
            "視差効果を減らす: {}\nコントラストを上げる: {}",
            onoff(p.reduce_motion),
            onoff(p.increase_contrast)
        ),
    }
}

/// 失敗時のみ非空。
pub fn error_text(state: &AccessibilityPrefsShowState) -> String {
    match &state.view {
        PrefsView::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 注記（「不明」のときの見え方と反映の遅れ）。
pub const NOTE: &str = "設定が不明・取得できないときは、丸は動き、枠は通常の太さです。設定の変更は最大 1 秒ほどで反映されます。";

/// 「アクセシビリティ設定の追従」画面。
pub fn accessibility_prefs_show_screen() -> Screen<AccessibilityPrefsShowState> {
    Screen::new("アクセシビリティ設定の追従")
        .with_node(Node::Widget(Box::new(CanvasWidget::bound(
            CANVAS_ID,
            "動く丸",
            Measurement {
                width: FIELD_W,
                height: FIELD_H,
            },
            ball_commands,
        ))))
        .dynamic_text(status_text)
        .error_text(error_text)
        .text(NOTE)
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &AccessibilityPrefsShowState) -> Screen<AccessibilityPrefsShowState> {
    accessibility_prefs_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn read(rm: Option<bool>, ic: Option<bool>) -> Result<Prefs, ReadFailure> {
        Ok(Prefs {
            reduce_motion: rm,
            increase_contrast: ic,
        })
    }

    fn radii(s: &AccessibilityPrefsShowState) -> Vec<f32> {
        ball_commands(s)
            .iter()
            .filter_map(|c| match c {
                PaintCommand::Circle { radius, .. } => Some(*radius),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn non_read_states_never_say_on_and_keep_animating() {
        let others = [
            PrefsView::Reading,
            PrefsView::Unsupported,
            PrefsView::Failed("設定の取得".to_string()),
            PrefsView::Read(Prefs::default()),
        ];
        for view in others {
            let s = AccessibilityPrefsShowState { phase: 0.0, view };
            assert!(!status_text(&s).contains("オン"), "{:?}", s.view);
            assert!(animates(&s));
            assert_eq!(border_width(&s), NORMAL_BORDER);
        }
    }

    #[test]
    fn unknown_values_show_unknown() {
        let mut s = AccessibilityPrefsShowState::default();
        apply_read(&mut s, read(None, None));
        assert_eq!(
            status_text(&s),
            "視差効果を減らす: 不明\nコントラストを上げる: 不明"
        );
    }

    #[test]
    fn reduce_motion_on_stops_ball() {
        let mut s = AccessibilityPrefsShowState::default();
        apply_read(&mut s, read(Some(true), Some(false)));
        assert!(!animates(&s));
        apply_read(&mut s, read(Some(false), Some(false)));
        assert!(animates(&s));
    }

    #[test]
    fn border_width_follows_contrast() {
        let mut s = AccessibilityPrefsShowState::default();
        apply_read(&mut s, read(Some(false), Some(true)));
        assert_eq!(border_width(&s), CONTRAST_BORDER);
        apply_read(&mut s, read(Some(false), Some(false)));
        assert_eq!(border_width(&s), NORMAL_BORDER);
    }

    #[test]
    fn outer_circle_is_drawn_first_with_border_added() {
        let mut s = AccessibilityPrefsShowState::default();
        apply_read(&mut s, read(Some(false), Some(true)));
        assert_eq!(radii(&s), vec![BALL_RADIUS + CONTRAST_BORDER, BALL_RADIUS]);
        assert!(matches!(
            ball_commands(&s)[0],
            PaintCommand::StyledRect { .. }
        ));
    }

    #[test]
    fn failure_drops_previous_read() {
        for r in [
            Err(ReadFailure::Unsupported),
            Err(ReadFailure::Failed("設定の取得".to_string())),
        ] {
            let mut s = AccessibilityPrefsShowState::default();
            apply_read(&mut s, read(Some(true), Some(true)));
            apply_read(&mut s, r);
            assert!(!matches!(s.view, PrefsView::Read(_)));
            assert!(animates(&s));
            assert_eq!(border_width(&s), NORMAL_BORDER);
        }
    }

    #[test]
    fn phase_wraps_and_ball_stays_inside() {
        let mut s = AccessibilityPrefsShowState::default();
        for _ in 0..200 {
            advance_phase(&mut s, 0.1);
            assert!((0.0..1.0).contains(&s.phase));
            if let PaintCommand::Circle { center, .. } = ball_commands(&s)[1] {
                assert!(center.0 >= BALL_RADIUS && center.0 <= FIELD_W - BALL_RADIUS);
            }
        }
    }

    #[test]
    fn failed_sets_error_text() {
        let mut s = AccessibilityPrefsShowState::default();
        apply_read(&mut s, Err(ReadFailure::Failed("設定の取得".to_string())));
        assert_eq!(error_text(&s), "失敗しました: 設定の取得");
        apply_read(&mut s, read(None, None));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn render_headless_does_not_panic() {
        let s = AccessibilityPrefsShowState::default();
        let joined = render_headless(&accessibility_prefs_show_screen(), &s).join("\n");
        assert!(joined.contains("読み取っています"));
    }
}
