// ケース 2: 所有値を `move` キャプチャするクロージャを `ReadAccessor::new` へ渡す。
// 参照キャプチャ（ケース 1）だけでなく所有権キャプチャも同様に `fn` へ coerce できず
// 拒否されるべきことを確認する（REQ-13 / NFR-9）。
use fandhe_core::state::ReadAccessor;

struct CounterState {
    count: i32,
}

fn main() {
    let label = String::from("count");
    let accessor: ReadAccessor<CounterState, i32> =
        ReadAccessor::new(move |state| state.count + label.len() as i32);
    let _ = accessor;
}
