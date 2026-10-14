// ケース 7: 所有 `String` を `move` キャプチャするクロージャを `Node::TextField` の
// `set: fn(&mut S, String)` フィールドへ設定する。書き込み側フィールドも同様に
// クロージャの環境キャプチャを拒否することを確認する（REQ-13 / NFR-9）。
use fandhe_core::Node;

struct CounterState {
    note: String,
}

fn main() {
    let log_prefix = String::from("更新: ");
    let node: Node<CounterState> = Node::TextField {
        label: "メモ".to_string(),
        get: |state| state.note.clone(),
        set: move |state, value| state.note = format!("{log_prefix}{value}"),
    };
    let _ = node;
}
