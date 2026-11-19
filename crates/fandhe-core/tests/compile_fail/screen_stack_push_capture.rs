// ケース: 環境をキャプチャするクロージャを `ScreenStack::push` へ渡す（Issue #1289）。
// 構築関数が `fn(&S) -> Screen<S>` になっても、クロージャは `fn` ポインタへ coerce できず
// 拒否されるべきことを確認する（REQ-13 / NFR-9）。
use fandhe_core::{Screen, ScreenStack};

struct AppState;

fn root(_: &AppState) -> Screen<AppState> {
    Screen::new("root")
}

fn main() {
    let title = String::from("detail");
    let mut stack: ScreenStack<AppState> = ScreenStack::new(root);
    stack.push(move |_state| Screen::new(title.as_str()));
}
