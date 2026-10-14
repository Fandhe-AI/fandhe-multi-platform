// ケース 1: 外部変数を参照キャプチャするクロージャを `ReadAccessor::new` へ渡す。
// `ReadAccessor::new` の引数型は `ReadFn<S, T> = fn(&S) -> T` であり、
// 環境をキャプチャするクロージャは `fn` ポインタへ coerce できないため拒否されるべき
// （REQ-13 / NFR-9、TASK-13.1-S1 の `state::ReadAccessor` が対象）。
use fandhe_core::state::ReadAccessor;

struct CounterState {
    count: i32,
}

fn main() {
    let bonus = 10;
    let accessor: ReadAccessor<CounterState, i32> = ReadAccessor::new(|state| state.count + bonus);
    let _ = accessor;
}
