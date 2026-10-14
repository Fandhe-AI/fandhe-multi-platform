//! ヒットテスト → ポインタディスパッチ → `StateStore::dispatch` の一気通貫結合テスト
//! （TASK-8.3-S3 / REQ-8、Issue #98）。
//!
//! `Screen<S>` にボタンを 1 つ宣言し、対応する `HitRegion` を手動で組んだ上で、
//! 座標クリック（Down→Up）が実際に `StateStore` の状態更新・`version` 増加まで
//! 到達することを検証する。矩形の算出（レイアウトプロトコル、Issue #99）自体は
//! 本 issue のスコープ外のため、`HitRegion` はテスト内で決め打ちの矩形を用いて構築する。

use fandhe_core::{
    Event, HitRegion, Point, PointerDispatcher, PointerEvent, PointerEventKind, Rect, Screen, Size,
    StateStore, WidgetId,
};

/// カウンタ状態。ボタン押下で 1 増える。
#[derive(Default)]
struct Counter {
    count: u32,
}

fn increment(state: &mut Counter) {
    state.count += 1;
}

fn button_screen() -> Screen<Counter> {
    Screen::new("home").button(WidgetId::from_raw(1), "increment", increment)
}

/// `increment` ボタンに対応する単一リージョンの矩形ツリー。
fn button_region() -> HitRegion {
    HitRegion::leaf(
        Rect::new(Point::new(0.0, 0.0), Size::new(20.0, 20.0)),
        "increment",
    )
}

#[test]
fn tap_inside_button_dispatches_click_and_updates_state() {
    let screen = button_screen();
    let region = button_region();
    let mut store = StateStore::new(Counter::default());
    let mut dispatcher = PointerDispatcher::new();

    let down = dispatcher.dispatch(
        &region,
        PointerEvent::new(PointerEventKind::Down, Point::new(10.0, 10.0)),
    );
    assert!(down.is_none(), "Down 単体では state::Event を発生させない");

    let up = dispatcher.dispatch(
        &region,
        PointerEvent::new(PointerEventKind::Up, Point::new(10.0, 10.0)),
    );
    let event = up.expect("同一ターゲット上の Down→Up はクリック確定するはず");
    assert_eq!(
        event,
        Event::ButtonClicked {
            label: "increment".to_string()
        }
    );

    let handled = store.dispatch(&screen, &event);

    assert!(handled);
    assert_eq!(store.state().count, 1);
    assert_eq!(store.version(), 1);
}

#[test]
fn drag_outside_button_before_release_does_not_update_state() {
    let screen = button_screen();
    let region = button_region();
    let mut store = StateStore::new(Counter::default());
    let mut dispatcher = PointerDispatcher::new();

    dispatcher.dispatch(
        &region,
        PointerEvent::new(PointerEventKind::Down, Point::new(10.0, 10.0)),
    );
    let up = dispatcher.dispatch(
        &region,
        PointerEvent::new(PointerEventKind::Up, Point::new(100.0, 100.0)),
    );

    assert!(up.is_none(), "ボタン外での Up はクリック確定しない");
    assert_eq!(store.state().count, 0);
    assert_eq!(store.version(), 0);

    // 念のため、Up が None の場合に dispatch を呼ばない運用（プラットフォームレイヤの
    // 想定契約）でも state が変化しないことを明示する。
    if let Some(event) = up {
        store.dispatch(&screen, &event);
    }
    assert_eq!(store.state().count, 0);
}

#[test]
fn up_without_down_produces_no_event() {
    let region = button_region();
    let mut dispatcher = PointerDispatcher::new();

    let up = dispatcher.dispatch(
        &region,
        PointerEvent::new(PointerEventKind::Up, Point::new(10.0, 10.0)),
    );

    assert!(up.is_none());
}

#[test]
fn cancel_after_down_prevents_click_on_subsequent_up() {
    let region = button_region();
    let mut dispatcher = PointerDispatcher::new();

    dispatcher.dispatch(
        &region,
        PointerEvent::new(PointerEventKind::Down, Point::new(10.0, 10.0)),
    );
    dispatcher.dispatch(
        &region,
        PointerEvent::new(PointerEventKind::Cancel, Point::new(10.0, 10.0)),
    );
    let up = dispatcher.dispatch(
        &region,
        PointerEvent::new(PointerEventKind::Up, Point::new(10.0, 10.0)),
    );

    assert!(up.is_none(), "Cancel 後のキャプチャなし Up は発火しない");
}
