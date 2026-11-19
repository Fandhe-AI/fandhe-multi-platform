//! `menu`（ドロップダウン）と `tabs` を並べた確認用画面（Issue #1429）。
//!
//! `FANDHE_PROXY_APP=menu-tabs` で選ばれ、`crates/fandhe-gallery/src/screens/registry.rs`
//! から呼ばれる（`screens/mod.rs` の `pub mod menu_tabs;` と対）。`fandhe-render-egui` が
//! 開閉・選択・タブの切り替えを状態へ書き戻すことの目視確認用。

use crate::{Screen, WidgetId};

/// メニューの開閉・選択とタブの選択を持つ状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MenuTabsState {
    /// メニューが開いているか。
    pub menu_open: bool,
    /// メニューで選んだ項目の index。
    pub picked: Option<usize>,
    /// 選択中のタブの index。
    pub tab: usize,
}

const SORTS: [&str; 3] = ["新しい順", "古い順", "名前順"];
const TABS: [&str; 3] = ["概要", "詳細", "履歴"];

fn menu_open(state: &MenuTabsState) -> bool {
    state.menu_open
}

fn set_menu_open(state: &mut MenuTabsState, open: bool) {
    state.menu_open = open;
}

fn picked(state: &MenuTabsState) -> Option<usize> {
    state.picked
}

fn set_picked(state: &mut MenuTabsState, index: usize) {
    state.picked = Some(index);
}

fn tab(state: &MenuTabsState) -> usize {
    state.tab
}

fn set_tab(state: &mut MenuTabsState, index: usize) {
    state.tab = index;
}

fn picked_text(state: &MenuTabsState) -> String {
    match state.picked.and_then(|i| SORTS.get(i)) {
        Some(sort) => format!("並び順: {sort}"),
        None => "並び順: 未選択".to_string(),
    }
}

/// 選択中のタブの中身。`tabs` は見出しだけを描くため、中身は状態を読む文字で出し分ける。
fn tab_body(state: &MenuTabsState) -> String {
    match TABS.get(state.tab) {
        Some(label) => format!("「{label}」タブの中身"),
        None => String::new(),
    }
}

/// メニュー・選択結果の文字・タブ・選択中タブの中身を縦に並べた画面を組み立てる。
pub fn menu_tabs_screen() -> Screen<MenuTabsState> {
    Screen::new("メニューとタブ")
        .menu(
            WidgetId::from_raw(1),
            "並び順を選ぶ",
            SORTS.iter().map(|s| (*s).to_string()).collect(),
            menu_open,
            set_menu_open,
            picked,
            set_picked,
        )
        .dynamic_text(picked_text)
        .tabs(
            WidgetId::from_raw(2),
            TABS.iter().map(|s| (*s).to_string()).collect(),
            tab,
            set_tab,
        )
        .dynamic_text(tab_body)
}
