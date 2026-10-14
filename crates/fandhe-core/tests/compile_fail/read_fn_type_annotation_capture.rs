// ケース 3: キャプチャ付きクロージャを `let f: ReadFn<S, T> = ...` へ直接束縛する。
// `ReadAccessor::new` 経由だけでなく、`ReadFn` 型注釈への直接代入でも
// 同様に拒否されることを確認する（型エイリアスの表記そのものが安全網であることの検証）。
use fandhe_core::state::ReadFn;

struct CounterState {
    count: i32,
}

fn main() {
    let threshold = 5;
    let f: ReadFn<CounterState, bool> = |state| state.count > threshold;
    let _ = f;
}
