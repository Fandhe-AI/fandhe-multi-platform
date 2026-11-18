//! 画面ルートのスクロール（Issue #77）の結合テスト。
//!
//! `render_screen` が見出し以降を縦スクロールで包むため、窓より長い本文の後ろにある
//! ボタンへもホイール操作で届くことを検証する。GPU を使わない既定の `Harness` で動く。
//! 実機（iPhone 横向き）の経路である指のドラッグも検証する。タッチのテストは
//! `with_step_dt(1/60)` が必須（既定の 0.25 秒だと押下直後に長押し判定が走りドラッグが止まる）。

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

/// 指で `from` から縦に `dy` ドラッグする（egui-winit の `on_touch` と同じ Touch + Pointer 同時送出）。
fn swipe(h: &mut egui_kittest::Harness<'_, StateStore<S>>, from: egui::Pos2, dy: f32) {
    let touch = |phase, pos| egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(0),
        phase,
        pos,
        force: None,
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    h.event(touch(egui::TouchPhase::Start, from));
    h.event(egui::Event::PointerMoved(from));
    h.event(button(from, true));
    h.run_steps(1);
    let steps = 10;
    let mut pos = from;
    for _ in 0..steps {
        pos.y += dy / steps as f32;
        h.event(touch(egui::TouchPhase::Move, pos));
        h.event(egui::Event::PointerMoved(pos));
        h.run_steps(1);
    }
    h.event(touch(egui::TouchPhase::End, pos));
    h.event(button(pos, false));
    h.event(egui::Event::PointerGone);
    // 慣性を切ってから次のドラッグへ進む。
    h.run_steps(30);
}

/// ボタン 6 個 + 200 行の VirtualList を指のドラッグだけで往復する（file_watch_show と同じ形）。
fn touch_scroll_reaches_everything(height: f32, outer_scrolls: bool) {
    let mut screen = Screen::new("指");
    for i in 0..6 {
        screen = screen.button(WidgetId::from_raw(i), format!("ボタン{i}"), |_s: &mut S| {});
    }
    let screen = screen.virtual_list(|_| 200, item, 24.0);
    let size = egui::vec2(800.0, height);
    let mut harness = egui_kittest::Harness::builder()
        .with_size(size)
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            move |ui, store: &mut StateStore<S>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(S),
        );
    harness.run();
    let visible = |h: &egui_kittest::Harness<'_, StateStore<S>>, label: &str| {
        h.query_by_label(label)
            .is_some_and(|n| n.rect().min.y >= 0.0 && n.rect().max.y <= size.y)
    };

    // ボタン横の余白のドラッグで外側が動く（高さ 340 は一覧が残りの高さに収まり動かない）。
    let before = harness.get_by_label("ボタン0").rect().min.y;
    swipe(&mut harness, egui::pos2(700.0, 40.0), -100.0);
    let moved = harness.get_by_label("ボタン0").rect().min.y != before;
    assert_eq!(moved, outer_scrolls, "外側スクロールの有無");

    // 行の文字の上のドラッグを繰り返して末尾へ。上限 400 回で打ち切る（無限ループ防止）。
    for n in 0.. {
        assert!(n < 400, "item-199 に届かない");
        if visible(&harness, "item-199") {
            break;
        }
        swipe(&mut harness, egui::pos2(20.0, size.y - 20.0), -100.0);
    }
    // 逆向きで先頭へ。
    for n in 0.. {
        assert!(n < 400, "item-0 に戻れない");
        if visible(&harness, "item-0") {
            break;
        }
        swipe(&mut harness, egui::pos2(20.0, size.y - 20.0), 100.0);
    }
    // ボタン横の余白のドラッグでボタンへ戻れる。
    for n in 0.. {
        assert!(n < 400, "ボタン0 に戻れない");
        if visible(&harness, "ボタン0") {
            break;
        }
        swipe(&mut harness, egui::pos2(700.0, 40.0), 100.0);
    }
    assert!(
        visible(&harness, "ボタン0"),
        "ボタン0 がクリップ内に戻らない"
    );
}

#[test]
fn touch_scroll_iphone_landscape_height() {
    touch_scroll_reaches_everything(340.0, false);
}

#[test]
fn touch_scroll_low_window() {
    touch_scroll_reaches_everything(200.0, true);
}
