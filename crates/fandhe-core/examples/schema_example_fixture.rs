//! `docs/ai/SCHEMA.md`「新規画面ファイルの置き場と `mod` 追加の作法」節の画面本体例
//! （`crates/fandhe-core/src/screens/schema_example_fixture.rs`）をヘッドレスで動作確認する
//! example。
//!
//! example は `fandhe-core` とは別クレートとしてコンパイルされ公開 API しか参照できない
//! ため、ここから実際に動作させることで「AI 生成画面が `examples/<name>.rs` から利用可能」
//! という SCHEMA.md の置き場規定が成立していることを固定化する（codex-review P1、
//! PR #668 3 巡目）。
//!
//! 実行方法: `cargo run -p fandhe-core --example schema_example_fixture`

use fandhe_core::click_button;
use fandhe_core::screens::schema_example_fixture::{TodoScreenState, todo_screen};

fn main() {
    let screen = todo_screen();
    let mut state = TodoScreenState::default();

    let handled = click_button(&screen, "追加", &mut state);

    println!("handled={handled}, items={:?}", state.items);
}
