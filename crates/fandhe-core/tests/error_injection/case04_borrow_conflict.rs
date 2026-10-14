// ケース04（PoC-6 case04 対応）: 借用違反（不変借用の存在下で可変借用を取得）。
// `StateStore::state()` が返す共有参照を保持したまま `StateStore::dispatch`（`&mut self`）
// を呼び出し、その後も共有参照を使用し続けるとコンパイル段階で拒否されることを確認する
// （E0502 想定）。
use fandhe_core::{Event, Screen, StateStore, WidgetId};

#[derive(Default)]
struct Counter {
    count: u32,
}

fn increment(state: &mut Counter) {
    state.count += 1;
}

fn main() {
    let screen: Screen<Counter> =
        Screen::new("home").button(WidgetId::from_raw(1), "increment", increment);
    let mut store = StateStore::new(Counter::default());

    let current = store.state();
    // `current` が生存中に `dispatch`（可変借用）を呼び出している
    store.dispatch(
        &screen,
        &Event::ButtonClicked {
            label: "increment".into(),
        },
    );
    println!("{}", current.count);
}
