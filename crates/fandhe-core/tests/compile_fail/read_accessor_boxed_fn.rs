// ケース 8: `Box<dyn Fn(&S) -> T>`（トレイトオブジェクト）を `ReadAccessor::new` へ渡す。
// クロージャの直接キャプチャだけでなく、動的ディスパッチによる遠回しな回避も
// `fn` ポインタ限定の型では受理されないことを確認する（REQ-13 / NFR-9）。
use fandhe_core::state::ReadAccessor;

struct CounterState {
    count: i32,
}

fn main() {
    let boxed: Box<dyn Fn(&CounterState) -> i32> = Box::new(|state| state.count);
    let accessor: ReadAccessor<CounterState, i32> = ReadAccessor::new(boxed);
    let _ = accessor;
}
