// ケース: 外部変数を参照キャプチャするクロージャを `Screen::text_input` の
// `get: fn(&S) -> String` 引数へ渡す。`.text_field()` と同様、新設した `.text_input()`
// ビルダー経由でもクロージャによる暗黙のキャプチャを拒否することを確認する
// （REQ-13 / NFR-9、TASK-8.3-S13 の `Screen::text_input`）。
use fandhe_core::{Screen, WidgetId};

struct FormState {
    note: String,
}

fn set_note(state: &mut FormState, value: String) {
    state.note = value;
}

fn main() {
    let prefix = String::from("メモ: ");
    let screen: Screen<FormState> = Screen::new("画面").text_input(
        WidgetId::from_raw(1),
        "メモ",
        |state| format!("{prefix}{}", state.note),
        set_note,
    );
    let _ = screen;
}
