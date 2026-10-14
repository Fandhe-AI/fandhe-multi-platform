// ケース 4（PoC-6 case05 踏襲）: 外部変数を `move` キャプチャするクロージャを
// `Screen::button` の `on_click: fn(&mut S)` へ渡す。
// `docs/spec/03-poc/ai-optimization/experiments/error-injection/src/bin/case05_closure_capture_as_fnptr.rs`
// で実証済みの拒否パターンを、実装コードベースの `ui` モジュール向けに踏襲する
// （REQ-13 / NFR-9、TASK-1.1-S3 の `Screen::button`）。
use fandhe_core::{Screen, WidgetId};

struct CounterState {
    count: i32,
}

fn main() {
    let bonus = 3;
    let screen: Screen<CounterState> =
        Screen::new("画面").button(WidgetId::from_raw(1), "加算", move |state| {
            state.count += bonus
        });
    let _ = screen;
}
