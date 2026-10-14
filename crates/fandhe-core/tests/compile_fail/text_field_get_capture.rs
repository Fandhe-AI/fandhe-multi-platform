// ケース 6: キャプチャ付きクロージャを `Node::TextField` の `get: fn(&S) -> String`
// フィールドへ設定する。`ui::Node` の各フィールドが構造体リテラル経由でも
// `fn` ポインタ限定であることを確認する（REQ-13 / NFR-9、TASK-1.1-S1/S3 の `Node::TextField`）。
use fandhe_core::Node;

struct CounterState {
    note: String,
}

fn main() {
    let prefix = String::from("メモ: ");
    let node: Node<CounterState> = Node::TextField {
        label: "メモ".to_string(),
        get: |state| format!("{prefix}{}", state.note),
        set: |state, value| state.note = value,
    };
    let _ = node;
}
