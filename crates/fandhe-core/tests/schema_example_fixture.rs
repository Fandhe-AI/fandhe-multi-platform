//! `docs/ai/SCHEMA.md`「新規画面ファイルの置き場と `mod` 追加の作法」節の画面本体例
//! （`crates/fandhe-core/src/screens/schema_example_fixture.rs`）の結合テスト。
//!
//! 結合テストは `fandhe-core` とは別クレートとしてコンパイルされ公開 API しか参照できない
//! ため、ここから実際に検証することで「AI 生成画面が `tests/<name>.rs` から利用可能」
//! という SCHEMA.md の置き場規定が成立していることを固定化する。

use fandhe_core::click_button;
use fandhe_core::screens::schema_example_fixture::{TodoScreenState, todo_screen};

#[test]
fn todo_screen_click_adds_item_from_external_crate() {
    let screen = todo_screen();
    let mut state = TodoScreenState::default();

    assert!(click_button(&screen, "追加", &mut state));
    assert_eq!(state.items, vec!["todo-1".to_string()]);
}
