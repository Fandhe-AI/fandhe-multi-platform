// 受理ケース 2: 環境をキャプチャしない（`fn` へ coerce 可能な）クロージャを
// `Screen::button` / `Screen::list` へ渡す。拒否ケース群（screen_button_capture.rs 等）と
// 対になる「正当な非キャプチャクロージャは通ること」の固定化（REQ-13 / NFR-9）。
use fandhe_core::{Node, Screen, WidgetId};

struct CounterState {
    count: i32,
}

fn main() {
    let screen = Screen::<CounterState>::new("画面")
        .button(WidgetId::from_raw(1), "加算", |state| state.count += 1)
        .list(|state| {
            (0..state.count)
                .map(|_| Node::Text("項目".to_string()))
                .collect()
        });
    assert_eq!(screen.len(), 2);
}
