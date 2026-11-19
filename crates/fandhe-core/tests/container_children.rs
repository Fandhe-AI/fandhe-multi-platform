//! `Stack` / `Grid` / `Card` / `Tooltip` の `Widget::children` と、子の操作が
//! `set_widget_value_at` の path で状態へ届くことの検証（Issue #1430）。
//!
//! path は `Screen` 直下の添字 + `Widget::children` の添字（`resolve_node_at` の降下規則）。

use fandhe_core::{
    Card, Checkbox, Grid, Screen, Stack, Tooltip, Widget, WidgetId, WidgetValue,
    set_widget_value_at,
};

#[derive(Default)]
struct St {
    a: bool,
    b: bool,
}

fn rd_a(s: &St) -> bool {
    s.a
}
fn wr_a(s: &mut St, v: bool) {
    s.a = v;
}
fn rd_b(s: &St) -> bool {
    s.b
}
fn wr_b(s: &mut St, v: bool) {
    s.b = v;
}
fn id(n: u64) -> WidgetId {
    WidgetId::from_raw(n)
}
fn cb_a() -> Checkbox<St> {
    Checkbox::bound(id(10), "a", rd_a, wr_a)
}
fn cb_b() -> Checkbox<St> {
    Checkbox::bound(id(11), "b", rd_b, wr_b)
}

#[test]
fn children_return_children_in_declaration_order() {
    let st = St::default();
    let stack = Stack::new(id(1)).with_child(cb_a()).with_child(cb_b());
    let grid = Grid::new(id(2)).with_child(cb_a()).with_child(cb_b());
    let card = Card::new(id(3)).with_child(cb_a()).with_child(cb_b());
    assert_eq!(stack.children(&st).len(), 2);
    assert_eq!(grid.children(&st).len(), 2);
    assert_eq!(card.children(&st).len(), 2);
    assert_eq!(
        Tooltip::fixed(id(4), "t")
            .with_anchor(cb_a())
            .children(&st)
            .len(),
        1
    );
    assert!(Tooltip::<St>::fixed(id(4), "t").children(&st).is_empty());
    assert!(Stack::<St>::new(id(5)).children(&st).is_empty());
}

/// 2 番目の子（path `[0, 1]`）の値変更が状態へ届く。
#[test]
fn child_value_change_reaches_state_through_each_container() {
    let value = WidgetValue::Bool(true);
    let screens = [
        Screen::new("t").stack(Stack::new(id(1)).with_child(cb_a()).with_child(cb_b())),
        Screen::new("t").grid(Grid::new(id(1)).with_child(cb_a()).with_child(cb_b())),
        Screen::new("t").card(Card::new(id(1)).with_child(cb_a()).with_child(cb_b())),
    ];
    for screen in &screens {
        let mut st = St::default();
        assert!(set_widget_value_at(
            screen,
            &[0, 1],
            id(11),
            &value,
            &mut st
        ));
        assert!(st.b && !st.a);
    }
    let tip = Screen::new("t").with_node(fandhe_core::Node::Widget(Box::new(
        Tooltip::fixed(id(1), "t").with_anchor(cb_a()),
    )));
    let mut st = St::default();
    assert!(set_widget_value_at(&tip, &[0, 0], id(10), &value, &mut st));
    assert!(st.a);
}

/// 無効化した `Card` は子へ解決できず、操作が届かない（`DialogWidget` と同じ契約）。
#[test]
fn disabled_card_hides_children_from_path_resolution() {
    let value = WidgetValue::Bool(true);
    let build = |disabled: bool| {
        let mut card = Card::new(id(1)).with_child(cb_a());
        card.set_disabled(disabled);
        Screen::new("t").card(card)
    };
    let mut st = St::default();
    assert!(set_widget_value_at(
        &build(false),
        &[0, 0],
        id(10),
        &value,
        &mut st
    ));
    assert!(st.a);

    let mut st = St::default();
    assert!(!set_widget_value_at(
        &build(true),
        &[0, 0],
        id(10),
        &value,
        &mut st
    ));
    assert!(!st.a);
}
