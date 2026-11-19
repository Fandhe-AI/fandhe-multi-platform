//! `Node::Row` / `Node::CellButton` の実描画と、`Node::List` の `axis` の並びを
//! egui_kittest で検証する（Issue #1288）。
//!
//! `widget_render.rs` と違い wgpu の描画器は使わない（レイアウトと入力の検証だけで、
//! 画素を読まないため。アダプタ欠如でスキップされない）。

use egui_kittest::kittest::Queryable;
use fandhe_core::{Axis, Node, Screen, StateStore, WidgetId};

#[derive(Default)]
struct St {
    opened: Vec<usize>,
    deleted: Vec<usize>,
    rated: Option<(usize, usize)>,
}

fn open(s: &mut St, row: usize) {
    s.opened.push(row);
}
fn delete(s: &mut St, row: usize) {
    s.deleted.push(row);
}
fn rate(s: &mut St, cell: (usize, usize)) {
    s.rated = Some(cell);
}

fn row(i: usize, tap: Option<usize>) -> Node<St> {
    let base = 10 * i as u64;
    Node::Row {
        children: vec![
            Node::IndexedButton {
                label: format!("開く{i}"),
                index: i,
                on_click: open,
                id: WidgetId::from_raw(base + 1),
            },
            Node::CellButton {
                label: format!("★{i}"),
                row: i,
                col: 3,
                on_click: rate,
                id: WidgetId::from_raw(base + 2),
            },
            Node::IndexedButton {
                label: format!("削除{i}"),
                index: i,
                on_click: delete,
                id: WidgetId::from_raw(base + 3),
            },
        ],
        tap,
    }
}

fn rows_tap(_: &St) -> Vec<Node<St>> {
    (0..3).map(|i| row(i, Some(0))).collect()
}
fn rows_no_tap(_: &St) -> Vec<Node<St>> {
    (0..3).map(|i| row(i, None)).collect()
}

fn harness(screen: Screen<St>) -> egui_kittest::Harness<'static, StateStore<St>> {
    egui_kittest::Harness::builder()
        .with_size(egui::vec2(500.0, 400.0))
        .build_ui_state(
            move |ui, store: &mut StateStore<St>| {
                fandhe_render_egui::render_screen(ui, &screen, store);
            },
            StateStore::new(St::default()),
        )
}

/// 座標を直接クリックする（ノードを持たない行の空き部分用）。
fn click_at(h: &mut egui_kittest::Harness<'static, StateStore<St>>, pos: egui::Pos2) {
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
}

fn rows_harness(
    expand: fn(&St) -> Vec<Node<St>>,
) -> egui_kittest::Harness<'static, StateStore<St>> {
    let mut h = harness(Screen::new("t").list(expand));
    h.run();
    h
}

#[test]
fn row_buttons_sit_side_by_side() {
    let h = rows_harness(rows_tap);
    let a = h.get_by_label("開く0").rect();
    let b = h.get_by_label("★0").rect();
    let c = h.get_by_label("削除0").rect();
    assert!((a.center().y - b.center().y).abs() < 1.0);
    assert!((b.center().y - c.center().y).abs() < 1.0);
    assert!(a.max.x <= b.min.x + 1.0 && b.max.x <= c.min.x + 1.0);
    // 2 行目は 1 行目より下。
    assert!(h.get_by_label("開く1").rect().center().y > a.center().y + 5.0);
}

#[test]
fn only_second_row_delete_fires() {
    let mut h = rows_harness(rows_tap);
    h.get_by_label("削除1").click();
    h.run();
    assert_eq!(h.state().state().deleted, vec![1]);
    assert!(h.state().state().opened.is_empty());
}

#[test]
fn cell_button_passes_row_and_col() {
    let mut h = rows_harness(rows_tap);
    h.get_by_label("★1").click();
    h.run();
    assert_eq!(h.state().state().rated, Some((1, 3)));
}

#[test]
fn tap_on_empty_row_area_fires_entry_once() {
    let mut h = rows_harness(rows_tap);
    let r = h.get_by_label("削除1").rect();
    // 行の右端の空き部分（ボタンの外）。
    click_at(&mut h, egui::pos2(480.0, r.center().y));
    h.run();
    assert_eq!(h.state().state().opened, vec![1]);
    assert!(h.state().state().deleted.is_empty());
    assert_eq!(h.state().state().rated, None);
}

#[test]
fn clicking_inner_button_does_not_fire_row_entry() {
    let mut h = rows_harness(rows_tap);
    h.get_by_label("削除2").click();
    h.run();
    assert!(h.state().state().opened.is_empty());
    assert_eq!(h.state().state().deleted, vec![2]);
}

#[test]
fn tap_none_ignores_empty_area() {
    let mut h = rows_harness(rows_no_tap);
    let r = h.get_by_label("削除1").rect();
    click_at(&mut h, egui::pos2(480.0, r.center().y));
    h.run();
    let s = h.state().state();
    assert!(s.opened.is_empty() && s.deleted.is_empty() && s.rated.is_none());
}

fn four(_: &St) -> Vec<Node<St>> {
    "ABCD".chars().map(|c| Node::Text(c.to_string())).collect()
}

fn axis_harness(axis: Axis) -> egui_kittest::Harness<'static, StateStore<St>> {
    let mut h = harness(Screen::new("t").list_with_axis(four, axis));
    h.run();
    h
}

#[test]
fn list_horizontal_places_items_on_one_line() {
    let h = axis_harness(Axis::Horizontal);
    let ys: Vec<f32> = "ABCD"
        .chars()
        .map(|c| h.get_by_label(&c.to_string()).rect().center().y)
        .collect();
    assert!(ys.iter().all(|y| (y - ys[0]).abs() < 1.0), "{ys:?}");
    let xs: Vec<f32> = "ABCD"
        .chars()
        .map(|c| h.get_by_label(&c.to_string()).rect().min.x)
        .collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
}

#[test]
fn list_grid_has_two_columns() {
    let h = axis_harness(Axis::Grid { columns: 2 });
    let r = |c: &str| h.get_by_label(c).rect();
    // A B / C D
    assert!((r("A").center().y - r("B").center().y).abs() < 1.0);
    assert!((r("C").center().y - r("D").center().y).abs() < 1.0);
    assert!(r("C").center().y > r("A").center().y + 5.0);
    assert!((r("A").min.x - r("C").min.x).abs() < 1.0);
    assert!(r("B").min.x > r("A").min.x);
}

#[test]
fn list_vertical_stacks_items() {
    let h = axis_harness(Axis::Vertical);
    let ys: Vec<f32> = "ABCD"
        .chars()
        .map(|c| h.get_by_label(&c.to_string()).rect().min.y)
        .collect();
    assert!(ys.windows(2).all(|w| w[0] < w[1]), "{ys:?}");
}

fn text_row(_: &St) -> Vec<Node<St>> {
    vec![Node::Row {
        children: vec![
            Node::IndexedButton {
                label: "開く".to_string(),
                index: 7,
                on_click: open,
                id: WidgetId::from_raw(1),
            },
            Node::Text("説明の文字".to_string()),
        ],
        tap: Some(0),
    }]
}

/// 文字（Label）は既定で選択可能でクリックを自分で受けるため、行タップが届くよう
/// 行の中では選択不可にしている。
#[test]
fn tap_on_text_child_fires_entry_once() {
    let mut h = rows_harness(text_row);
    let c = h.get_by_label("説明の文字").rect().center();
    click_at(&mut h, c);
    assert_eq!(h.state().state().opened, vec![7]);
}

#[test]
fn clicking_entry_button_itself_fires_once() {
    let mut h = rows_harness(text_row);
    h.get_by_label("開く").click();
    h.run();
    assert_eq!(h.state().state().opened, vec![7]);
}

fn long_row(_: &St) -> Vec<Node<St>> {
    let mut r = row(0, None);
    if let Node::Row { children, .. } = &mut r {
        children[0] = Node::IndexedButton {
            label: "とても長い題名とても長い題名とても長い題名".to_string(),
            index: 0,
            on_click: open,
            id: WidgetId::from_raw(1),
        };
    }
    vec![r]
}

/// 幅が狭くても、行内のボタンは折り返されて画面の中に残る。
#[test]
fn narrow_width_keeps_row_buttons_inside_screen() {
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(320.0, 400.0))
        .build_ui_state(
            move |ui, store: &mut StateStore<St>| {
                fandhe_render_egui::render_screen(
                    ui,
                    &Screen::new("t").list_with_axis(long_row, Axis::Vertical),
                    store,
                );
            },
            StateStore::new(St::default()),
        );
    h.run();
    for label in ["★0", "削除0"] {
        let r = h.get_by_label(label).rect();
        assert!(r.max.x <= 320.0, "{label}: {r:?}");
    }
}
