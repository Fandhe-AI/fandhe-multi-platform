//! 画面ルートのスクロール（Issue #77）の結合テスト。
//!
//! `render_screen` が見出し以降を縦スクロールで包むため、窓より長い本文の後ろにある
//! ボタンへもホイール操作で届くことを検証する。GPU を使わない既定の `Harness` で動く。

use egui_kittest::kittest::Queryable;
use fandhe_core::{Screen, StateStore, WidgetId};

#[derive(Default)]
struct S;

#[test]
fn button_after_long_text_is_reachable_by_scrolling() {
    let long = (0..80)
        .map(|i| format!("行 {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let screen = Screen::new("長い画面").text(long).button(
        WidgetId::from_raw(1),
        "末尾ボタン",
        |_s: &mut S| {},
    );
    let size = egui::vec2(300.0, 200.0);
    let mut harness = egui_kittest::Harness::builder()
        .with_size(size)
        .build_ui_state(
            move |ui, store: &mut StateStore<S>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(S),
        );
    harness.run();
    assert!(harness.get_by_label("末尾ボタン").rect().min.y > size.y);

    // ホイールはポインタが乗っている領域にだけ効く。
    harness.event(egui::Event::PointerMoved(egui::pos2(150.0, 100.0)));
    harness.run();
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -5000.0),
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(30);
    assert!(harness.get_by_label("末尾ボタン").rect().max.y <= size.y);
}

fn len(_: &S) -> usize {
    300
}

fn item(_: &S, i: usize) -> fandhe_core::Node<S> {
    fandhe_core::Node::Text(format!("item-{i}"))
}

/// ボタンで窓を使い切っても、その下の VirtualList の行へ外側スクロールで届く。
#[test]
fn virtual_list_rows_are_reachable_below_many_buttons() {
    let mut screen = Screen::new("低い窓");
    for i in 0..6 {
        screen = screen.button(WidgetId::from_raw(i), format!("ボタン{i}"), |_s: &mut S| {});
    }
    let screen = screen.virtual_list(len, item, 40.0);
    let size = egui::vec2(300.0, 200.0);
    let mut harness = egui_kittest::Harness::builder()
        .with_size(size)
        .build_ui_state(
            move |ui, store: &mut StateStore<S>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(S),
        );
    harness.run();
    harness.event(egui::Event::PointerMoved(egui::pos2(150.0, 60.0)));
    harness.run();
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -5000.0),
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(30);
    let row = harness.get_by_label("item-0").rect();
    assert!(row.max.y <= size.y && row.min.y >= 0.0, "{row:?}");
}
