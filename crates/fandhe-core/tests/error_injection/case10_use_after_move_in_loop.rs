// ケース10（PoC-6 case10 対応）: ループ内での use-after-move（非 Copy 値を複数回
// ムーブしようとする）。`WriteAccessor::set` は値 `T` を所有権ごと受け取るため、
// `String` のような非 Copy 値を `.clone()` せずループ内で繰り返し渡すと
// コンパイル段階で拒否されることを確認する（E0382 想定）。
use fandhe_core::state::WriteAccessor;

struct FormState {
    memo: String,
}

fn set_memo(state: &mut FormState, value: String) {
    state.memo = value;
}

fn main() {
    let accessor: WriteAccessor<FormState, String> = WriteAccessor::new(set_memo);
    let mut state = FormState {
        memo: String::new(),
    };
    let value = "更新".to_string();

    for _ in 0..3 {
        // `value` は 1 回目のループでムーブ済みのため 2 回目以降はコンパイルエラー（`.clone()` が必要）
        accessor.set(&mut state, value);
    }
}
