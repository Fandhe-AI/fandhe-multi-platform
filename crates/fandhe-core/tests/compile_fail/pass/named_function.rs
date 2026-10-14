// 受理ケース 1: 名前付き関数を `ReadAccessor::new` へ渡す。
// 拒否ケース群が過剰拒否になっていないこと（正当な使い方は通ること）を固定化する
// （REQ-13 / NFR-9、TASK-13.1-S1 の `ReadAccessor` の想定用途そのもの）。
use fandhe_core::state::ReadAccessor;

struct CounterState {
    count: i32,
}

fn read_count(state: &CounterState) -> i32 {
    state.count
}

fn main() {
    let accessor: ReadAccessor<CounterState, i32> = ReadAccessor::new(read_count);
    let state = CounterState { count: 1 };
    assert_eq!(accessor.get(&state), 1);
}
