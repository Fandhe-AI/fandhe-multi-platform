//! 型サンプル「チェックボックス一覧」（親 #1036、Issue #1037）の結合テスト。
//! `render_headless` の行内容を assert し、初期値・行の個別切り替え・一括操作が
//! `checkbox_list_screen()` の実際の描画・イベント配線を通して成立することを検証する。
//! `Node::Widget`（Checkbox）は `render_headless` が行を出力しない契約
//! （`ui/render.rs` 参照）のため、値変更は `set_widget_value_at` で駆動し、
//! 結果は下部の集約テキスト（`summary`）で検証する（`tests/settings.rs` と同じ方針）。

use fandhe_core::screens::checkbox_list::{CheckboxListState, checkbox_list_screen};
use fandhe_core::state::WidgetValue;
use fandhe_core::ui::set_widget_value_at;
use fandhe_core::{WidgetId, click_button, render_headless};

/// 行 i のトップレベルパス（`checkbox_list_screen` の組み立て順、`List` は
/// トップレベル index 1）。
fn row_path(i: usize) -> [usize; 2] {
    [1, i]
}

/// 行 i に割り当てた `WidgetId`（`checkbox_list.rs` の `ROW_ID_BASE = 100`）。
fn row_id(i: usize) -> WidgetId {
    WidgetId::from_raw(100 + i as u64)
}

/// 初期状態のヘッドレス描画が、要件どおりの初期値（先頭 3 件オン）を反映している
/// ことを検証する。
#[test]
fn initial_render_shows_defaults() {
    let screen = checkbox_list_screen();
    let state = CheckboxListState::default();
    let lines = render_headless(&screen, &state);

    assert_eq!(lines[0], "# チェックボックス一覧");
    assert!(lines.contains(&"有効: 3 / 5 件（基本辞書, 人名辞書, 地名辞書）".to_string()));
    assert!(lines.contains(&"(すべて選択)".to_string()));
    assert!(lines.contains(&"(すべて解除)".to_string()));
}

/// 行の個別切り替えが集約テキストへ往復して反映される。
#[test]
fn toggle_row_updates_summary() {
    let screen = checkbox_list_screen();
    let mut state = CheckboxListState::default();

    // 行 3（顔文字辞書）をオンにする（初期値はオフ）。
    assert!(set_widget_value_at(
        &screen,
        &row_path(3),
        row_id(3),
        &WidgetValue::Bool(true),
        &mut state,
    ));
    let lines = render_headless(&screen, &state);
    assert!(
        lines.contains(&"有効: 4 / 5 件（基本辞書, 人名辞書, 地名辞書, 顔文字辞書）".to_string())
    );

    // 戻す。
    assert!(set_widget_value_at(
        &screen,
        &row_path(3),
        row_id(3),
        &WidgetValue::Bool(false),
        &mut state,
    ));
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"有効: 3 / 5 件（基本辞書, 人名辞書, 地名辞書）".to_string()));
}

/// 「すべて選択」「すべて解除」が集約テキストへ反映され、その後の個別操作も続けて
/// 効くことを検証する。
#[test]
fn select_all_and_clear_all_update_summary() {
    let screen = checkbox_list_screen();
    let mut state = CheckboxListState::default();

    assert!(click_button(&screen, "すべて選択", &mut state));
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(
        &"有効: 5 / 5 件（基本辞書, 人名辞書, 地名辞書, 顔文字辞書, 郵便番号辞書）".to_string()
    ));

    assert!(click_button(&screen, "すべて解除", &mut state));
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"有効: 0 / 5 件（なし）".to_string()));

    assert!(set_widget_value_at(
        &screen,
        &row_path(0),
        row_id(0),
        &WidgetValue::Bool(true),
        &mut state,
    ));
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"有効: 1 / 5 件（基本辞書）".to_string()));
}

/// 行の path に別の行の `WidgetId` を渡すと id ガードで拒否され、状態が変わらない
/// （id 照合による取り違え防止、`device-bridge-contracts.md` チェック表 2 参照）。
#[test]
fn wrong_id_is_rejected() {
    let screen = checkbox_list_screen();
    let mut state = CheckboxListState::default();
    let before = state;

    assert!(!set_widget_value_at(
        &screen,
        &row_path(3),
        row_id(4),
        &WidgetValue::Bool(true),
        &mut state,
    ));
    assert_eq!(state, before);
}
