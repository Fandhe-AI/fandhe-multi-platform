//! 型サンプル「設定画面」（Issue #727 / #807）の結合テスト。`render_headless` の行内容を
//! assert し、初期値・各操作・clamp・初期値へのリセットが `settings_screen()` の
//! 実際の描画・イベント配線を通して成立することを検証する。`Node::Widget`
//! （トグル・スライダー・ラジオボタン）は `render_headless` が行を出力しない契約
//! （`ui/render.rs` 参照）のため、値変更は `set_widget_value_at` で駆動し、
//! 結果は下部の集約テキスト（`summary`）で検証する。

use fandhe_core::screens::settings::{SettingsState, set_font_size, settings_screen};
use fandhe_core::state::WidgetValue;
use fandhe_core::ui::set_widget_value_at;
use fandhe_core::{WidgetId, click_button, render_headless};

/// トップレベルのツリー上の位置（`settings_screen` の組み立て順、`screens/settings.rs` 参照）。
const NOTIFICATIONS_TOGGLE_PATH: [usize; 1] = [0];
const FONT_SIZE_SLIDER_PATH: [usize; 1] = [1];
const THEME_SYSTEM_RADIO_PATH: [usize; 1] = [3];
const THEME_LIGHT_RADIO_PATH: [usize; 1] = [4];
const THEME_DARK_RADIO_PATH: [usize; 1] = [5];

/// 初期状態のヘッドレス描画が、要件どおりの初期値（通知オン・文字サイズ 16・
/// テーマ「端末に合わせる」）を反映していることを検証する。
#[test]
fn initial_render_shows_defaults() {
    let screen = settings_screen();
    let state = SettingsState::default();
    let lines = render_headless(&screen, &state);

    assert_eq!(lines[0], "# 設定");
    assert!(lines.contains(&"通知: オン / 文字サイズ: 16 / テーマ: 端末に合わせる".to_string()));
    assert!(lines.contains(&"(初期値に戻す)".to_string()));
}

/// 「通知」トグルの値変更が、下部の集約テキストへオフ→オンと往復して反映される。
#[test]
fn toggle_notifications_updates_summary() {
    let screen = settings_screen();
    let mut state = SettingsState::default();

    assert!(set_widget_value_at(
        &screen,
        &NOTIFICATIONS_TOGGLE_PATH,
        WidgetId::from_raw(1),
        &WidgetValue::Bool(false),
        &mut state,
    ));
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"通知: オフ / 文字サイズ: 16 / テーマ: 端末に合わせる".to_string()));

    assert!(set_widget_value_at(
        &screen,
        &NOTIFICATIONS_TOGGLE_PATH,
        WidgetId::from_raw(1),
        &WidgetValue::Bool(true),
        &mut state,
    ));
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"通知: オン / 文字サイズ: 16 / テーマ: 端末に合わせる".to_string()));
}

/// 文字サイズスライダーが `FONT_SIZE_MIN..=FONT_SIZE_MAX` の範囲で clamp され、
/// 小数値は丸められることを検証する（`apply_value` の clamp と `set_font_size` の
/// clamp の二重防御、`screens/settings.rs` 参照）。
#[test]
fn font_size_slider_clamps_and_rounds() {
    let screen = settings_screen();
    let mut state = SettingsState::default();

    assert!(set_widget_value_at(
        &screen,
        &FONT_SIZE_SLIDER_PATH,
        WidgetId::from_raw(2),
        &WidgetValue::Number(30.0),
        &mut state,
    ));
    assert!(
        render_headless(&screen, &state)
            .contains(&"通知: オン / 文字サイズ: 24 / テーマ: 端末に合わせる".to_string())
    );

    assert!(set_widget_value_at(
        &screen,
        &FONT_SIZE_SLIDER_PATH,
        WidgetId::from_raw(2),
        &WidgetValue::Number(5.0),
        &mut state,
    ));
    assert!(
        render_headless(&screen, &state)
            .contains(&"通知: オン / 文字サイズ: 12 / テーマ: 端末に合わせる".to_string())
    );

    assert!(set_widget_value_at(
        &screen,
        &FONT_SIZE_SLIDER_PATH,
        WidgetId::from_raw(2),
        &WidgetValue::Number(16.4),
        &mut state,
    ));
    assert!(
        render_headless(&screen, &state)
            .contains(&"通知: オン / 文字サイズ: 16 / テーマ: 端末に合わせる".to_string())
    );
}

/// テーマの 3 択ラジオボタンが排他的に選択状態を切り替えることを検証する。
#[test]
fn theme_radio_buttons_select_exclusively() {
    let screen = settings_screen();
    let mut state = SettingsState::default();

    assert!(set_widget_value_at(
        &screen,
        &THEME_LIGHT_RADIO_PATH,
        WidgetId::from_raw(4),
        &WidgetValue::Index(1),
        &mut state,
    ));
    assert!(
        render_headless(&screen, &state)
            .contains(&"通知: オン / 文字サイズ: 16 / テーマ: ライト".to_string())
    );

    assert!(set_widget_value_at(
        &screen,
        &THEME_DARK_RADIO_PATH,
        WidgetId::from_raw(5),
        &WidgetValue::Index(2),
        &mut state,
    ));
    assert!(
        render_headless(&screen, &state)
            .contains(&"通知: オン / 文字サイズ: 16 / テーマ: ダーク".to_string())
    );

    assert!(set_widget_value_at(
        &screen,
        &THEME_SYSTEM_RADIO_PATH,
        WidgetId::from_raw(3),
        &WidgetValue::Index(0),
        &mut state,
    ));
    assert!(
        render_headless(&screen, &state)
            .contains(&"通知: オン / 文字サイズ: 16 / テーマ: 端末に合わせる".to_string())
    );
}

/// 各種操作後に「初期値に戻す」を押すと、状態・集約テキストとも初期値へ復帰する。
#[test]
fn reset_restores_defaults() {
    let screen = settings_screen();
    let mut state = SettingsState::default();

    assert!(set_widget_value_at(
        &screen,
        &NOTIFICATIONS_TOGGLE_PATH,
        WidgetId::from_raw(1),
        &WidgetValue::Bool(false),
        &mut state,
    ));
    assert!(set_widget_value_at(
        &screen,
        &FONT_SIZE_SLIDER_PATH,
        WidgetId::from_raw(2),
        &WidgetValue::Number(20.0),
        &mut state,
    ));
    assert!(set_widget_value_at(
        &screen,
        &THEME_DARK_RADIO_PATH,
        WidgetId::from_raw(5),
        &WidgetValue::Index(2),
        &mut state,
    ));
    assert!(click_button(&screen, "初期値に戻す", &mut state));

    assert_eq!(state, SettingsState::default());
    assert!(
        render_headless(&screen, &state)
            .contains(&"通知: オン / 文字サイズ: 16 / テーマ: 端末に合わせる".to_string())
    );
}

/// 未知のボタンラベルは処理されず `false` を返すことを検証する。
#[test]
fn unknown_button_is_not_handled() {
    let screen = settings_screen();
    let mut state = SettingsState::default();

    assert!(!click_button(&screen, "存在しない", &mut state));
}

/// 公開 setter を直接 `NaN` / `∞` で呼んでも範囲契約が破れず現在値を維持する
/// （PR #810 codex-review P1）。
#[test]
fn set_font_size_ignores_non_finite_values() {
    let mut state = SettingsState::default();
    let before = state.font_size();

    set_font_size(&mut state, f32::NAN);
    assert_eq!(state.font_size(), before);

    set_font_size(&mut state, f32::INFINITY);
    assert_eq!(state.font_size(), before);

    set_font_size(&mut state, f32::NEG_INFINITY);
    assert_eq!(state.font_size(), before);
}
