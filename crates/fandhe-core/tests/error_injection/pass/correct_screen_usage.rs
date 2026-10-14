// 過剰拒否ガード: 上記 12 ケースが打ち消す誤りを含まない、正しい API 使用が
// コンパイル通過することを固定化する（TASK-13.3、`compile_fail` の裏付けとして
// `pass` 側にも 1 ケースを置く既存 `tests/compile_fail.rs` の構成を踏襲）。
// `Screen` / `Node` / `state::{ReadAccessor, WriteAccessor}` / `StateStore` を
// 正しい引数・型で組み合わせて使用する。
use fandhe_core::state::{ReadAccessor, WriteAccessor};
use fandhe_core::{Event, Screen, StateStore, WidgetId};

#[derive(Default)]
struct FormState {
    memo: String,
}

fn get_memo(state: &FormState) -> String {
    state.memo.clone()
}

fn set_memo(state: &mut FormState, value: String) {
    state.memo = value;
}

fn clear_memo(state: &mut FormState) {
    state.memo.clear();
}

fn main() {
    let get_accessor: ReadAccessor<FormState, String> = ReadAccessor::new(get_memo);
    let set_accessor: WriteAccessor<FormState, String> = WriteAccessor::new(set_memo);

    let screen: Screen<FormState> = Screen::new("フォーム")
        .text("メモを入力してください")
        .text_field("メモ", get_accessor.as_fn(), set_memo)
        .button(WidgetId::from_raw(1), "クリア", clear_memo);

    let mut store = StateStore::new(FormState::default());
    set_accessor.set(&mut FormState::default(), "初期値".to_string());

    let handled = store.dispatch(
        &screen,
        &Event::TextFieldChanged {
            label: "メモ".to_string(),
            value: "更新後".to_string(),
        },
    );
    assert!(handled);
    assert_eq!(store.state().memo, "更新後");

    let handled = store.dispatch(
        &screen,
        &Event::ButtonClicked {
            label: "クリア".to_string(),
        },
    );
    assert!(handled);
    assert_eq!(store.state().memo, "");
}
