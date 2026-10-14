// ケース: 所有値を `move` キャプチャするクロージャを `WriteAccessor::new` へ渡す。
// 読み取り側（`read_accessor_move_capture.rs`）と対称に、書き込み側アクセサも
// 環境をキャプチャするクロージャを `fn` へ coerce できず拒否されるべきことを
// 確認する（REQ-13 / NFR-9、#307: Node::TextField::set の WriteFn 統合に伴う追補）。
use fandhe_core::state::WriteAccessor;

struct CounterState {
    count: i32,
}

fn main() {
    let step = 2;
    let accessor: WriteAccessor<CounterState, i32> =
        WriteAccessor::new(move |state, value| state.count = value + step);
    let _ = accessor;
}
