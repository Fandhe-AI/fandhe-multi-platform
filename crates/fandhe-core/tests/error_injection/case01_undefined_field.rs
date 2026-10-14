// ケース01（PoC-6 case01 対応）: 未定義プロパティ参照（フィールド名のタイプミス）。
// `fandhe_core::state::ReadFn` 経由の読み取り関数内で、状態構造体に存在しない
// フィールドを参照するとコンパイル段階で拒否されることを確認する（E0609 想定）。
use fandhe_core::state::ReadFn;

struct LoginState {
    email: String,
}

fn get_email(state: &LoginState) -> String {
    // `mail` は存在しないフィールド（正しくは `email`）
    state.mail.clone()
}

fn main() {
    let f: ReadFn<LoginState, String> = get_email;
    let _ = f;
}
