//! `render_screen` が `Node` の各種・各 `Widget` を実際に egui の描画（shape）へ出すかを、
//! GPU を使わない `egui::Context::run` で 1 種ずつ確かめる（Issue #1290）。
//!
//! 結果は `docs/ai/SCHEMA.md`「egui で描画される部品・されない部品」の表の根拠。
//! 描画されない部品は、空画面と同じ shape 数のまま（何も出ない）であることを固定する。
//! 将来その部品の描画を配線したらここが失敗するので、期待を `true` へ直し SCHEMA.md も更新する。

use fandhe_core::ui::{FlexDirection, Measurement};
use fandhe_core::{
    CanvasWidget, Card, DialogWidget, Grid, Label, PaintCommand, RgbaPixels, Screen, Separator,
    Stack, StateStore, WidgetId,
};

#[derive(Default)]
struct St {
    flag: bool,
    num: usize,
    f: f32,
    text: String,
}

fn rd_bool(s: &St) -> bool {
    s.flag
}
fn wr_bool(s: &mut St, v: bool) {
    s.flag = v;
}
fn rd_usize(s: &St) -> usize {
    s.num
}
fn wr_usize(s: &mut St, v: usize) {
    s.num = v;
}
fn rd_f32(s: &St) -> f32 {
    s.f
}
fn wr_f32(s: &mut St, v: f32) {
    s.f = v;
}
fn rd_str(s: &St) -> String {
    s.text.clone()
}
fn wr_str(s: &mut St, v: String) {
    s.text = v;
}
fn rd_sel(_: &St) -> Option<usize> {
    Some(0)
}
fn rd_len(_: &St) -> usize {
    2
}
fn rd_item(_: &St, i: usize) -> String {
    format!("ITEM{i}")
}
fn rd_node(_: &St, i: usize) -> fandhe_core::Node<St> {
    fandhe_core::Node::Text(format!("VITEM{i}"))
}
fn rd_open(_: &St) -> bool {
    true
}
fn rd_cmds(_: &St) -> Vec<PaintCommand> {
    vec![PaintCommand::StyledRect {
        fill: fandhe_core::ColorValue {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        },
    }]
}
fn rd_label(_: &St) -> String {
    "BOUND".into()
}
fn rd_img(_: &St) -> String {
    "img-1290".into()
}
fn noop(_: &mut St) {}

fn id(n: u64) -> WidgetId {
    WidgetId::from_raw(n)
}

/// 画面を数フレーム描画し、(出た shape の総数, 出た文字列) を返す。
fn paint(screen: Screen<St>) -> (usize, String) {
    let ctx = egui::Context::default();
    let mut store = StateStore::new(St::default());
    let mut out = (0, String::new());
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 800.0),
            )),
            ..Default::default()
        };
        let full = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                fandhe_render_egui::render_screen(ui, &screen, &mut store);
            });
        });
        let mut count = 0;
        let mut text = String::new();
        for clipped in &full.shapes {
            walk(&clipped.shape, &mut count, &mut text);
        }
        out = (count, text);
    }
    out
}

fn walk(shape: &egui::Shape, count: &mut usize, text: &mut String) {
    match shape {
        egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, count, text)),
        egui::Shape::Noop => {}
        egui::Shape::Text(t) => {
            *count += 1;
            text.push_str(t.galley.text());
            text.push('|');
        }
        _ => *count += 1,
    }
}

#[test]
fn rendered_or_not() {
    // `.image()` が描かれず `ImageWidget` だけ描かれることを見るため、識別子を保管所へ置く。
    fandhe_render_egui::image_store::put(
        "img-1290".into(),
        RgbaPixels::new(1, 1, vec![255u8; 4]).unwrap(),
    );
    let (base, _) = paint(Screen::new("empty"));
    // (名前, 画面, 描画されるべきか)
    let cases: Vec<(&str, Screen<St>, bool)> = vec![
        ("Node::Text", Screen::new("t").text("X"), true),
        (
            "Node::TextField",
            Screen::new("t").text_field("X", rd_str, wr_str),
            true,
        ),
        (
            "Node::Button",
            Screen::new("t").button(id(1), "X", noop),
            true,
        ),
        (
            "Node::DynamicText",
            Screen::new("t").dynamic_text(rd_label),
            true,
        ),
        (
            "Node::StyledText",
            Screen::new("t").error_text(rd_label),
            true,
        ),
        (
            "Node::List",
            Screen::new("t").list(|_| vec![fandhe_core::Node::Text("X".into())]),
            true,
        ),
        (
            "Node::VirtualList",
            Screen::new("t").virtual_list(rd_len, rd_node, 20.0),
            true,
        ),
        (
            "Node::Row",
            Screen::new("t").with_node(fandhe_core::Node::Row {
                children: vec![fandhe_core::Node::Text("X".into())],
                tap: None,
            }),
            true,
        ),
        (
            "Node::CellButton",
            Screen::new("t").with_node(fandhe_core::Node::CellButton {
                label: "X".into(),
                row: 0,
                col: 0,
                on_click: |_, _| {},
                id: id(1),
            }),
            true,
        ),
        (
            "Node::RgbaImage",
            Screen::new("t").rgba_image(RgbaPixels::new(1, 1, vec![255u8; 4]).unwrap(), "X"),
            true,
        ),
        (
            "Checkbox",
            Screen::new("t").checkbox(id(1), "X", rd_bool, wr_bool),
            true,
        ),
        (
            "ToggleSwitch",
            Screen::new("t").toggle_switch(id(1), "X", rd_bool, wr_bool),
            true,
        ),
        (
            "RadioButton",
            Screen::new("t").radio_button(id(1), "X", 0, rd_usize, wr_usize),
            true,
        ),
        (
            "Slider",
            Screen::new("t").slider(id(1), "X", 0.0, 1.0, rd_f32, wr_f32),
            true,
        ),
        (
            "DialogWidget(new_dynamic)",
            Screen::new("t").with_node(fandhe_core::Node::Widget(Box::new(
                DialogWidget::new_dynamic(id(1), "X", "Y", "Z", true, rd_open, wr_bool),
            ))),
            true,
        ),
        (
            "CanvasWidget",
            Screen::new("t").with_node(fandhe_core::Node::Widget(Box::new(CanvasWidget::bound(
                id(1),
                "X",
                Measurement {
                    width: 50.0,
                    height: 50.0,
                },
                rd_cmds,
            )))),
            true,
        ),
        (
            "ImageWidget(store 一致)",
            Screen::new("t").with_node(fandhe_core::Node::Widget(Box::new(
                fandhe_core::ImageWidget::<St>::fixed(id(1), "img-1290", "X"),
            ))),
            true,
        ),
        (
            "Image(.image())",
            Screen::new("t").image(id(1), "img-1290", "X"),
            false,
        ),
        (
            "Image(.image_bound())",
            Screen::new("t").image_bound(id(1), rd_img, "X"),
            false,
        ),
        (
            "Icon(.icon_bound())",
            Screen::new("t").icon_bound(id(1), rd_img, "X"),
            false,
        ),
        ("Label", Screen::new("t").label(id(1), rd_label), false),
        (
            "TextInput",
            Screen::new("t").text_input(id(1), "X", rd_str, wr_str),
            false,
        ),
        (
            "ListView",
            Screen::new("t").list_view(id(1), rd_len, rd_item),
            false,
        ),
        (
            "ProgressBar",
            Screen::new("t").progress_bar(id(1), "X", rd_f32),
            false,
        ),
        ("Spinner", Screen::new("t").spinner(id(1)), false),
        ("Icon", Screen::new("t").icon(id(1), "star", "X"), false),
        (
            "ScrollView",
            Screen::new("t").scroll_view(id(1), Label::<St>::fixed(id(2), "X")),
            false,
        ),
        (
            "Stack",
            Screen::new("t").stack(
                Stack::new(id(1))
                    .with_direction(FlexDirection::Column)
                    .with_child(Label::fixed(id(2), "X")),
            ),
            false,
        ),
        (
            "Grid",
            Screen::new("t").grid(
                Grid::new(id(1))
                    .with_columns(2)
                    .with_child(Label::fixed(id(2), "X")),
            ),
            false,
        ),
        (
            "Card",
            Screen::new("t").card(Card::new(id(1)).with_child(Label::fixed(id(2), "X"))),
            false,
        ),
        (
            "Separator",
            Screen::new("t").separator(Separator::new(id(1))),
            false,
        ),
        (
            "Dialog",
            Screen::new("t").dialog(id(1), "X", "Y", rd_open),
            false,
        ),
        (
            "Menu",
            Screen::new("t").menu(id(1), "X", vec!["Y".into()], rd_open, rd_sel),
            false,
        ),
        (
            "Tabs",
            Screen::new("t").tabs(id(1), vec!["X".into()], rd_usize),
            false,
        ),
        (
            "Tooltip",
            Screen::new("t").tooltip(id(1), "Y", Label::<St>::fixed(id(2), "X")),
            false,
        ),
    ];
    let mut report = String::new();
    let mut bad = Vec::new();
    for (name, screen, expect) in cases {
        let (n, text) = paint(screen);
        let drawn = n > base;
        report.push_str(&format!("{name}: shapes={n} (base {base}) text={text}\n"));
        if drawn != expect {
            bad.push(name);
        }
    }
    println!("{report}");
    assert!(bad.is_empty(), "期待と実測が食い違う: {bad:?}\n{report}");
}
