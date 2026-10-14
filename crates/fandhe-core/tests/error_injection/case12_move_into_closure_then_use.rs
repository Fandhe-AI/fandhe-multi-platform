// ケース12（PoC-6 case12 対応・素の Rust のまま据え置き）: クロージャへのムーブ後に
// 元の変数を使用する（所有権違反、E0382 想定）。`fandhe_core` の関数ポインタ限定 API
// （`ReadFn`/`WriteFn`）へ move クロージャを渡すと、狙った use-after-move（E0382）より
// 先に「fn ポインタへ coerce 不能」という別種のエラー（E0308、`case05` と同じ分類）が
// 発生し複合エラーになってしまうため、本ケースは意図的に `fandhe_core` API を経由せず
// 素の Rust のまま据え置く（`error_injection.rs` の責務分界コメント、
// PoC-6 の元ケースからの差分）。
fn main() {
    let items: Vec<String> = vec!["タスク1".to_string(), "タスク2".to_string()];
    let closure = move || items.len();
    println!("{}", closure());
    // `items` はクロージャへムーブ済みのため使用できない
    println!("{:?}", items);
}
