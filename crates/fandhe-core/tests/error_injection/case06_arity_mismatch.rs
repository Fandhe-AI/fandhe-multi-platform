// ケース06（PoC-6 case06 対応）: 引数個数の不一致。`Screen::text_field` は
// label/get/set の 3 引数を取るが、余分な第 4 引数を渡すとコンパイル段階で
// 拒否されることを確認する（E0061 想定）。
use fandhe_core::Screen;

struct FormState {
    memo: String,
}

fn get_memo(state: &FormState) -> String {
    state.memo.clone()
}

fn set_memo(state: &mut FormState, value: String) {
    state.memo = value;
}

fn main() {
    // text_field は 3 引数（label/get/set）だが、余分な第 4 引数 "extra" を渡している
    let screen: Screen<FormState> =
        Screen::new("フォーム").text_field("メモ", get_memo, set_memo, "extra");
    let _ = screen;
}
