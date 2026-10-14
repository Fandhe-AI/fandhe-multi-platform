// ケース11（PoC-6 case11 対応）: ジェネリック状態型の不一致（`Screen<LoginState>` を
// `Screen<TodoState>` として扱う）。`Screen<S>` は状態型 `S` に対してジェネリックであり、
// 異なる状態型の `Screen` を混同するとコンパイル段階で拒否されることを確認する
// （E0308 想定、PoC-6 で実証済みの型安全性を維持）。
use fandhe_core::Screen;

#[derive(Default)]
struct LoginState {
    email: String,
}

#[derive(Default)]
struct TodoState {
    items: Vec<String>,
}

fn render_todo_screen(_screen: &Screen<TodoState>) {}

fn main() {
    let login_screen: Screen<LoginState> = Screen::new("ログイン");
    // LoginState 用の画面を TodoState 用の関数に渡している（型不一致）
    render_todo_screen(&login_screen);
}
