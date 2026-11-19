//! `Screen::menu` / `Screen::tabs` を egui で描き、クリックが状態の変化まで届くことを
//! egui_kittest で検証する（Issue #1429）。
//!
//! 経路は 描画（`render.rs` の `Node::Widget` 分岐）→ `Event::WidgetValueChanged`
//! （開閉は `WidgetValue::Bool`、選択は `WidgetValue::Index`）→ `Widget::apply_value` →
//! 利用者の `WriteFn`。`row_render.rs` と同じく wgpu は使わない。

use egui_kittest::kittest::Queryable;
use fandhe_core::{Screen, StateStore, WidgetId};

#[derive(Default)]
struct St {
    open: bool,
    /// `set_open` が呼ばれた回数。
    open_writes: usize,
    backed: bool,
    picked: Option<usize>,
    tab: usize,
}

fn rd_open(s: &St) -> bool {
    s.open
}
fn wr_open(s: &mut St, v: bool) {
    s.open = v;
    s.open_writes += 1;
}
fn rd_picked(s: &St) -> Option<usize> {
    s.picked
}
fn wr_picked(s: &mut St, i: usize) {
    s.picked = Some(i);
}
fn rd_tab(s: &St) -> usize {
    s.tab
}
fn wr_tab(s: &mut St, i: usize) {
    s.tab = i;
}

fn harness(screen: Screen<St>) -> egui_kittest::Harness<'static, StateStore<St>> {
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(400.0, 300.0))
        .build_ui_state(
            move |ui, store: &mut StateStore<St>| {
                fandhe_render_egui::render_screen(ui, &screen, store);
            },
            StateStore::new(St::default()),
        );
    h.run();
    h
}

fn menu_harness() -> egui_kittest::Harness<'static, StateStore<St>> {
    harness(Screen::new("t").menu(
        WidgetId::from_raw(1),
        "並び順",
        vec!["新しい順".into(), "古い順".into()],
        rd_open,
        wr_open,
        rd_picked,
        wr_picked,
    ))
}

#[test]
fn menu_trigger_opens_and_item_selects_and_closes() {
    let mut h = menu_harness();
    // 閉じている間は項目が出ない。
    assert!(h.query_by_label("古い順").is_none());

    h.get_by_label("並び順").click();
    h.run();
    assert!(h.state().state().open, "トリガーで開く");
    h.get_by_label("新しい順");

    h.get_by_label("古い順").click();
    h.run();
    assert_eq!(h.state().state().picked, Some(1));
    assert!(!h.state().state().open, "選ぶと閉じる");
    // 閉じると項目は消え、トリガーは選んだ項目の文言になる。
    assert!(h.query_by_label("新しい順").is_none());
    // 閉じたあと、トリガーは選んだ項目の文言を表示する（macOS・Android・Windows では
    // AX の名前はメニューのラベルのまま、選んだ項目は value。下の AX テストで確認）。
    #[cfg(not(any(target_os = "macos", target_os = "android", target_os = "windows")))]
    h.get_by_label("古い順");
}

#[test]
fn menu_trigger_toggles_closed() {
    let mut h = menu_harness();
    h.get_by_label("並び順").click();
    h.run();
    assert!(h.state().state().open);
    h.get_by_label("並び順").click();
    h.run();
    assert!(!h.state().state().open, "もう一度押すと閉じる");
    assert_eq!(h.state().state().picked, None);
}

#[test]
fn menu_outside_click_closes_without_selecting() {
    let mut h = menu_harness();
    h.get_by_label("並び順").click();
    h.run();
    assert!(h.state().state().open);
    let pos = egui::pos2(380.0, 280.0);
    h.event(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
    }
    assert!(!h.state().state().open, "外側クリックで閉じる");
    assert_eq!(h.state().state().picked, None);
}

#[test]
fn tabs_click_switches_selected_tab() {
    let mut h = harness(Screen::new("t").tabs(
        WidgetId::from_raw(1),
        vec!["概要".into(), "詳細".into(), "履歴".into()],
        rd_tab,
        wr_tab,
    ));
    h.get_by_label("履歴").click();
    h.run();
    assert_eq!(h.state().state().tab, 2);
    h.get_by_label("詳細").click();
    h.run();
    assert_eq!(h.state().state().tab, 1);
}

/// AX は egui 自身のノードを補正して 1 個だけ出す（合成ノードと二重にならない）。
#[cfg(any(target_os = "macos", target_os = "android", target_os = "windows"))]
#[test]
fn ax_roles_are_combo_box_and_tab_without_duplicates() {
    use egui::accesskit::Role;
    use egui_kittest::kittest::NodeT;
    let mut h = menu_harness();
    assert_eq!(h.get_all_by_label("並び順").count(), 1);
    h.get_by_role(Role::ComboBox);
    // 選んだあとも名前はラベルのまま、選んだ項目は value。
    h.get_by_label("並び順").click();
    h.run();
    h.get_by_label("古い順").click();
    h.run();
    let combo = h.get_by_role(Role::ComboBox);
    assert_eq!(combo.accesskit_node().label().as_deref(), Some("並び順"));
    assert_eq!(combo.accesskit_node().value().as_deref(), Some("古い順"));
    h.get_by_label("並び順").click();
    h.run();
    assert_eq!(
        h.get_by_role(Role::ComboBox)
            .accesskit_node()
            .data()
            .is_expanded(),
        Some(true)
    );

    let h = harness(Screen::new("t").tabs(
        WidgetId::from_raw(1),
        vec!["概要".into(), "詳細".into()],
        rd_tab,
        wr_tab,
    ));
    assert_eq!(h.get_all_by_role(Role::Tab).count(), 2);
    assert_eq!(h.get_all_by_label("概要").count(), 1);
}

#[test]
fn trigger_click_while_open_writes_open_exactly_once() {
    let mut h = menu_harness();
    h.get_by_label("並び順").click();
    h.run();
    assert_eq!(h.state().state().open_writes, 1);
    // 開いているときのトリガー押下は閉じる 1 回だけ。Popup の外側クリック扱いで二重に積まない。
    h.get_by_label("並び順").click();
    h.run();
    assert!(!h.state().state().open);
    assert_eq!(h.state().state().open_writes, 2);
}

fn on_back(s: &mut St) {
    s.backed = true;
}

/// 実際の app と同じ順（描画 → `dispatch_back_if_requested`）で、開いたメニューの Esc が
/// 閉じるだけで戻るへ漏れないことを確かめる。
#[test]
fn escape_closes_menu_without_back() {
    let screen = Screen::new("t")
        .menu(
            WidgetId::from_raw(1),
            "並び順",
            vec!["新しい順".into(), "古い順".into()],
            rd_open,
            wr_open,
            rd_picked,
            wr_picked,
        )
        .on_back(on_back);
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(400.0, 300.0))
        .build_ui_state(
            move |ui, store: &mut StateStore<St>| {
                fandhe_render_egui::render_screen(ui, &screen, store);
                fandhe_render_egui::dispatch_back_if_requested(ui.ctx(), &screen, store);
            },
            StateStore::new(St::default()),
        );
    h.run();
    h.get_by_label("並び順").click();
    h.run();
    assert!(h.state().state().open);

    h.key_press(egui::Key::Escape);
    h.run();
    assert!(!h.state().state().open, "Esc で閉じる");
    assert!(!h.state().state().backed, "戻るは発火しない");
}

fn rd_always_open(_: &St) -> bool {
    true
}

fn always_open_menu(disabled: bool) -> egui_kittest::Harness<'static, StateStore<St>> {
    let mut menu = fandhe_core::Menu::bound(
        WidgetId::from_raw(1),
        "並び順",
        vec!["新しい順".into()],
        rd_always_open,
        wr_open,
        rd_picked,
        wr_picked,
    );
    menu.set_disabled(disabled);
    harness(Screen::new("t").with_node(fandhe_core::Node::Widget(Box::new(menu))))
}

/// 無効化中は開いていてもポップアップを出さない（操作も閉じることもできないため）。
#[test]
fn disabled_open_menu_draws_no_popup() {
    assert!(always_open_menu(false).query_by_label("新しい順").is_some());
    assert!(always_open_menu(true).query_by_label("新しい順").is_none());
}
