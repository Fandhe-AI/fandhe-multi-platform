//! `Stack` / `Grid` / `Card` / `Tooltip` の子を egui で描き、子の操作が状態へ届くことを
//! egui_kittest で検証する（Issue #1430）。
//!
//! 経路は 再帰描画（`render.rs` の `Node::Widget` 分岐 → `render_children`）→
//! `Event::WidgetValueChanged { path }`（path は `Widget::children` の添字の列）→
//! `set_widget_value_at` → 利用者の `WriteFn`。子には `Checkbox`（Widget 側で描かれる部品）を使う。
//! 子 `Label`（`.label()` 系）は #1427 の結論待ちで描かれないため、ここでは使わない。

use egui_kittest::kittest::Queryable;
use fandhe_core::ui::FlexDirection;
use fandhe_core::{Card, Checkbox, Grid, Node, Screen, Stack, StateStore, Tabs, Tooltip, WidgetId};

#[derive(Default)]
struct St {
    a: bool,
    b: bool,
    c: bool,
    tab: usize,
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
fn rd_c(s: &St) -> bool {
    s.c
}
fn wr_c(s: &mut St, v: bool) {
    s.c = v;
}

fn rd_tab(s: &St) -> usize {
    s.tab
}
fn wr_tab(s: &mut St, v: usize) {
    s.tab = v;
}
fn tabs() -> Tabs<St> {
    Tabs::bound(id(50), vec!["T1".into(), "T2".into()], rd_tab, wr_tab)
}

fn id(n: u64) -> WidgetId {
    WidgetId::from_raw(n)
}
fn cb_a() -> Checkbox<St> {
    Checkbox::bound(id(10), "甲", rd_a, wr_a)
}
fn cb_b() -> Checkbox<St> {
    Checkbox::bound(id(11), "乙", rd_b, wr_b)
}
fn cb_c() -> Checkbox<St> {
    Checkbox::bound(id(12), "丙", rd_c, wr_c)
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

#[test]
fn stack_column_stacks_children_and_child_click_reaches_state() {
    let mut h =
        harness(Screen::new("t").stack(Stack::new(id(1)).with_child(cb_a()).with_child(cb_b())));
    let (a, b) = (h.get_by_label("甲").rect(), h.get_by_label("乙").rect());
    assert!(b.min.y > a.min.y + 5.0, "縦に並ぶ");
    h.get_by_label("乙").click();
    h.run();
    assert!(
        h.state().state().b && !h.state().state().a,
        "2 番目の子の path で届く"
    );
}

#[test]
fn stack_row_places_children_side_by_side() {
    let h = harness(
        Screen::new("t").stack(
            Stack::new(id(1))
                .with_direction(FlexDirection::Row)
                .with_spacing(20.0)
                .with_child(cb_a())
                .with_child(cb_b()),
        ),
    );
    let (a, b) = (h.get_by_label("甲").rect(), h.get_by_label("乙").rect());
    assert!((a.center().y - b.center().y).abs() < 1.0);
    assert!(b.min.x > a.max.x, "横に並ぶ");
}

#[test]
fn grid_places_children_in_columns_and_child_click_reaches_state() {
    let mut h = harness(
        Screen::new("t").grid(
            Grid::new(id(1))
                .with_columns(2)
                .with_child(cb_a())
                .with_child(cb_b())
                .with_child(cb_c()),
        ),
    );
    let (a, b, c) = (
        h.get_by_label("甲").rect(),
        h.get_by_label("乙").rect(),
        h.get_by_label("丙").rect(),
    );
    assert!(
        (a.center().y - b.center().y).abs() < 1.0 && b.min.x > a.max.x,
        "1 行目は 2 列"
    );
    assert!(c.min.y > a.max.y - 1.0, "3 つ目は次の行");
    h.get_by_label("丙").click();
    h.run();
    assert!(h.state().state().c);
}

#[test]
fn card_draws_frame_and_nested_child_click_reaches_state() {
    // Stack の中の Card の中の Checkbox（path は [0, 0, 0]）。
    let screen = Screen::new("t")
        .stack(Stack::new(id(1)).with_child(Card::new(id(2)).with_padding(8.0).with_child(cb_a())));
    let mut h = harness(screen);
    let h_plain = harness(Screen::new("t").stack(Stack::new(id(1)).with_child(cb_a())));
    // 枠の分だけ子が内側へずれる。
    assert!(h.get_by_label("甲").rect().min.x > h_plain.get_by_label("甲").rect().min.x + 4.0);
    h.get_by_label("甲").click();
    h.run();
    assert!(h.state().state().a);
}

#[test]
fn disabled_card_blocks_child_click() {
    let mut card = Card::new(id(2)).with_child(cb_a());
    card.set_disabled(true);
    let mut h = harness(Screen::new("t").card(card));
    h.get_by_label("甲");
    h.get_by_label("甲").click();
    h.run();
    assert!(!h.state().state().a);
}

fn tooltip_screen() -> Screen<St> {
    Screen::new("t").with_node(fandhe_core::Node::Widget(Box::new(
        Tooltip::fixed(id(3), "補足説明").with_anchor(cb_a()),
    )))
}

/// 本文を出しているノードの数。合成 AX ノード（macOS・Android・Windows のみ）が本文を持つため、
/// 「0 か 1 か」ではなく重ねる前後の差で、egui のバブルが出たかを見る。
fn bubbles(h: &egui_kittest::Harness<'static, StateStore<St>>) -> usize {
    h.query_all_by_label("補足説明").count()
}

fn hover_anchor(h: &mut egui_kittest::Harness<'static, StateStore<St>>) {
    let pos = h.get_by_label("甲").rect().center();
    h.event(egui::Event::PointerMoved(pos));
    for _ in 0..30 {
        h.step();
    }
}

#[test]
fn tooltip_shows_on_hover_only_and_anchor_click_reaches_state() {
    let mut h = harness(tooltip_screen());
    let before = bubbles(&h);
    hover_anchor(&mut h);
    assert_eq!(bubbles(&h), before + 1, "重ねると出る");
    h.event(egui::Event::PointerMoved(egui::pos2(380.0, 290.0)));
    for _ in 0..30 {
        h.step();
    }
    assert_eq!(bubbles(&h), before, "外れると消える");
    h.get_by_label("甲").click();
    h.run();
    assert!(h.state().state().a, "アンカーの操作は状態へ届く");
}

#[test]
fn disabled_tooltip_does_not_show() {
    let mut tip = Tooltip::fixed(id(3), "補足説明").with_anchor(cb_a());
    tip.set_disabled(true);
    let mut h = harness(Screen::new("t").with_node(fandhe_core::Node::Widget(Box::new(tip))));
    let before = bubbles(&h);
    hover_anchor(&mut h);
    assert_eq!(bubbles(&h), before);
}

fn touch(h: &mut egui_kittest::Harness<'static, StateStore<St>>, pos: egui::Pos2, pressed: bool) {
    // eframe（winit）はタッチをポインタの移動・ボタンへ写す。離したときは位置も消す。
    h.event(egui::Event::PointerMoved(pos));
    h.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
    if !pressed {
        h.event(egui::Event::PointerGone);
    }
}

/// タッチ端末の扱い（render.rs の Tooltip 分岐の doc と同じ）: 本文のバブルは出さない。
/// egui は押している間（押したまま止めても）hover 扱いにせず、離すとポインタ自体が消えるため、
/// タップでも長押しでも出ない。操作できるアンカー・できないアンカーのどちらでも同じ。
#[test]
fn tooltip_on_touch_never_shows_bubble() {
    fn rd_half(_: &St) -> f32 {
        0.5
    }
    let passive = Screen::new("t").with_node(fandhe_core::Node::Widget(Box::new(
        Tooltip::fixed(id(3), "補足説明").with_anchor(fandhe_core::ProgressBar::bound(
            id(4),
            "進捗",
            rd_half,
        )),
    )));
    // アンカーの中心を押す（Linux では AX 名が「進捗 50%」になるので部分一致で引く）。
    for (screen, anchor) in [(passive, "進捗"), (tooltip_screen(), "甲")] {
        let mut h = harness(screen);
        let before = bubbles(&h);
        let pos = h.get_by_label_contains(anchor).rect().center();

        touch(&mut h, pos, true);
        for _ in 0..30 {
            h.step();
        }
        assert_eq!(bubbles(&h), before, "押し続けても出ない");
        touch(&mut h, pos, false);
        for _ in 0..30 {
            h.step();
        }
        assert_eq!(bubbles(&h), before, "離しても出ない");
    }
}

#[test]
fn shown_tooltip_stays_without_hover() {
    let mut tip = Tooltip::fixed(id(3), "補足説明").with_anchor(cb_a());
    tip.show();
    let plain = harness(tooltip_screen());
    let h = harness(Screen::new("t").with_node(fandhe_core::Node::Widget(Box::new(tip))));
    assert_eq!(bubbles(&h), bubbles(&plain) + 1);
}

fn tabs_gap(h: &egui_kittest::Harness<'static, StateStore<St>>) -> f32 {
    h.get_by_label("T2").rect().min.x - h.get_by_label("T1").rect().max.x
}

/// `Stack` の `with_spacing` は子の並びだけに効き、子の内部（`Tabs` の見出しの間隔）へは漏れない。
#[test]
fn stack_spacing_does_not_leak_into_children() {
    let top = harness(Screen::new("t").with_node(Node::Widget(Box::new(tabs()))));
    let in0 = harness(Screen::new("t").stack(Stack::new(id(1)).with_child(tabs())));
    let in24 =
        harness(Screen::new("t").stack(Stack::new(id(1)).with_spacing(24.0).with_child(tabs())));
    let want = tabs_gap(&top);
    assert!(want > 1.0, "上位に置くと見出しの間に隙間がある");
    assert_eq!(tabs_gap(&in0), want);
    assert_eq!(tabs_gap(&in24), want);
}

/// `Grid` も同じ（行・列の間隔は `Grid` が持ち、セルの中の `Card` の子の縦間隔へは漏れない）。
#[test]
fn grid_spacing_does_not_leak_into_children() {
    let card = || Card::new(id(2)).with_child(cb_a()).with_child(cb_b());
    let vgap = |h: &egui_kittest::Harness<'static, StateStore<St>>| {
        h.get_by_label("乙").rect().min.y - h.get_by_label("甲").rect().max.y
    };
    let top = harness(Screen::new("t").card(card()));
    let g = harness(Screen::new("t").grid(Grid::new(id(1)).with_spacing(24.0).with_child(card())));
    let in_stack = harness(Screen::new("t").stack(Stack::new(id(1)).with_child(card())));
    assert!(vgap(&top) > 1.0);
    assert_eq!(vgap(&g), vgap(&top));
    assert_eq!(vgap(&in_stack), vgap(&top));
}

/// `with_spacing` は子同士の間隔として効く。
#[test]
fn stack_spacing_separates_children() {
    let y = |sp: f32| {
        let h = harness(
            Screen::new("t").stack(
                Stack::new(id(1))
                    .with_spacing(sp)
                    .with_child(cb_a())
                    .with_child(cb_b()),
            ),
        );
        h.get_by_label("乙").rect().min.y - h.get_by_label("甲").rect().min.y
    };
    assert!((y(24.0) - y(0.0) - 24.0).abs() < 1.0);
}

/// 子の無いコンテナ・アンカーの無い `Tooltip` は余白を取らない。
#[test]
fn empty_containers_take_no_space() {
    let y = |h: &egui_kittest::Harness<'static, StateStore<St>>| h.get_by_label("甲").rect().min.y;
    let base = harness(Screen::new("t").with_node(Node::Widget(Box::new(cb_a()))));
    let stack = harness(
        Screen::new("t")
            .stack(Stack::<St>::new(id(1)))
            .with_node(Node::Widget(Box::new(cb_a()))),
    );
    let grid = harness(
        Screen::new("t")
            .grid(Grid::<St>::new(id(1)))
            .with_node(Node::Widget(Box::new(cb_a()))),
    );
    let tip = harness(
        Screen::new("t")
            .with_node(Node::Widget(Box::new(Tooltip::<St>::fixed(id(7), "x"))))
            .with_node(Node::Widget(Box::new(cb_a()))),
    );
    for h in [&stack, &grid, &tip] {
        assert_eq!(y(h), y(&base));
    }
}

/// 線の入った矩形（枠）の数。
fn stroked_rects(shape: &egui::Shape, n: &mut usize) {
    match shape {
        egui::Shape::Vec(v) => v.iter().for_each(|s| stroked_rects(s, n)),
        egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color.a() > 0 => *n += 1,
        _ => {}
    }
}

fn frames_for(screen: &Screen<St>) -> usize {
    let ctx = egui::Context::default();
    let mut store = StateStore::new(St::default());
    let mut n = 0;
    for _ in 0..2 {
        let full = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    fandhe_render_egui::render_screen(ui, screen, &mut store);
                });
            },
        );
        n = 0;
        full.shapes
            .iter()
            .for_each(|c| stroked_rects(&c.shape, &mut n));
    }
    n
}

/// `Card` は子の種類によらず枠（線つきの矩形）を描く。子が無い画面より 1 つ多い。
#[test]
fn card_draws_a_stroked_frame() {
    let base = frames_for(&Screen::new("t"));
    let card = Screen::new("t").card(
        Card::new(id(1))
            .with_padding(8.0)
            .with_child(fandhe_core::Label::fixed(id(2), "X")),
    );
    assert_eq!(frames_for(&card), base + 1);
}
