// ケース09（PoC-6 case09 対応・差分あり）: 戻り値型の不一致。PoC-6 の元ケースは
// `String` を返すべき関数が `u32` を返す単純な戻り値型不一致だが、本ケースは
// `fandhe_core::state::ReadFn` との統合を踏まえ「getter が値ではなく参照
// （`&String`）を返す」という PoC-6 実測の失敗モード（AI がありがちな取り違え）に
// 差し替えている（PoC-6 の元ケースからの差分、
// エラー分類は同じ E0308＝戻り値型不一致を維持）。
use fandhe_core::state::ReadFn;

struct FormState {
    memo: String,
}

fn get_memo(state: &FormState) -> &String {
    // ReadFn<FormState, String> は `fn(&FormState) -> String` を要求するが、
    // 値ではなく参照 `&String` を返しており戻り値型が一致しない
    &state.memo
}

fn main() {
    let f: ReadFn<FormState, String> = get_memo;
    let _ = f;
}
