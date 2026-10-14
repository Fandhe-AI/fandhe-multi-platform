// ケース 5: 外部 `Vec` を参照キャプチャするクロージャを `Screen::list` の
// `expand: fn(&S) -> Vec<Node<S>>` へ渡す。`List` の子要素展開も他バリアントと同様に
// クロージャによる暗黙のキャプチャを拒否することを確認する（REQ-1 / REQ-13 / NFR-9、
// TASK-1.1-S4 の `Screen::list`）。
use fandhe_core::{Node, Screen};

struct CounterState {
    count: i32,
}

fn main() {
    let extra_labels = vec!["追加".to_string()];
    let screen: Screen<CounterState> = Screen::new("画面").list(|_state| {
        extra_labels
            .iter()
            .map(|label| Node::Text(label.clone()))
            .collect()
    });
    let _ = screen;
}
