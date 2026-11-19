// Issue #1288 追加ケース: 外部変数を `move` キャプチャするクロージャを
// `Node::CellButton::on_click`（`WriteFn<S, (usize, usize)>`）へ渡す。
// `indexed_button_capture.rs` と同型。行・列を値で受ける対話ノードでも、環境を
// キャプチャするハンドラはコンパイルで弾かれる（REQ-13 / NFR-9）。
use fandhe_core::{Node, WidgetId};

struct CounterState {
    count: i32,
}

fn main() {
    let bonus = 3;
    let node: Node<CounterState> = Node::CellButton {
        label: "加算".to_string(),
        row: 0,
        col: 0,
        on_click: move |state, _cell| state.count += bonus,
        id: WidgetId::from_raw(1),
    };
    let _ = node;
}
