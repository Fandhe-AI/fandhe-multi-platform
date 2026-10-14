// Issue #491 追加ケース: 外部変数を `move` キャプチャするクロージャを
// `Node::IndexedButton::on_click`（`WriteFn<S, usize> = fn(&mut S, usize)`）へ渡す。
// `screen_button_capture.rs`（`Node::Button::on_click`）と同型のケースを、動的生成子
// 内の対話ノード用に新設した `Node::IndexedButton` 向けに追加する（REQ-13 / NFR-9）。
use fandhe_core::{Node, WidgetId};

struct CounterState {
    count: i32,
}

fn main() {
    let bonus = 3;
    let node: Node<CounterState> = Node::IndexedButton {
        label: "加算".to_string(),
        index: 0,
        on_click: move |state, _index| state.count += bonus,
        id: WidgetId::from_raw(1),
    };
    let _ = node;
}
