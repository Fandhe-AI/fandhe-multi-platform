// ケース 7: 外部変数を参照キャプチャするクロージャを `Screen::text_field` の
// `get: fn(&S) -> String` 引数へ渡す。`.button()` / `.list()` と同様、ビルダー経由
// でもクロージャによる暗黙のキャプチャを拒否することを確認する（REQ-13 / NFR-9、
// TASK-1.1-S2 の `Screen::text_field`）。
use fandhe_core::Screen;

struct CounterState {
    note: String,
}

fn set_note(state: &mut CounterState, value: String) {
    state.note = value;
}

fn main() {
    let prefix = String::from("メモ: ");
    let screen: Screen<CounterState> =
        Screen::new("画面").text_field("メモ", |state| format!("{prefix}{}", state.note), set_note);
    let _ = screen;
}
