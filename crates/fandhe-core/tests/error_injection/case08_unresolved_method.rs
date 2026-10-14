// ケース08（PoC-6 case08 対応）: 存在しないメソッド呼び出し（API の類推ミス）。
// `Screen` に存在しない `with_text_field`（正しくは `text_field`）を呼び出すと
// コンパイル段階で拒否されることを確認する（E0599 想定）。
use fandhe_core::Screen;

#[derive(Default)]
struct LoginState {
    email: String,
}

fn get_email(state: &LoginState) -> String {
    state.email.clone()
}

fn set_email(state: &mut LoginState, value: String) {
    state.email = value;
}

fn main() {
    // `with_text_field` という存在しないメソッド名を類推で使用（正しくは `text_field`）
    let _screen: Screen<LoginState> =
        Screen::new("ログイン").with_text_field("メールアドレス", get_email, set_email);
}
