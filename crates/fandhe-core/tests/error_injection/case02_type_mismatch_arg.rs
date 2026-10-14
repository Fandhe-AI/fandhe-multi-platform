// ケース02（PoC-6 case02 対応）: 型不整合（`ReadAccessor::new` へ戻り値型が異なる
// 関数ポインタを渡す）。`ReadAccessor<S, T>::new` の引数型は `ReadFn<S, T> = fn(&S) -> T`
// であり、`T` が一致しない関数ポインタは受理されないことを確認する（E0308 想定）。
use fandhe_core::state::ReadAccessor;

struct CounterState {
    count: i32,
}

fn get_count(state: &CounterState) -> i32 {
    state.count
}

fn main() {
    // get_count は `fn(&CounterState) -> i32` だが、`String` 読み取り用として宣言している
    let accessor: ReadAccessor<CounterState, String> = ReadAccessor::new(get_count);
    let _ = accessor;
}
