//! `docs/ai/SCHEMA.md`「新規画面ファイルの置き場と `mod` 追加の作法」節の画面本体例を、
//! 実際にこのクレート内の `screens/` 配下へ配置してコンパイル確認する参照実装
//! （PR #668 レビュー対応、Issue #653）。
//!
//! SCHEMA.md 本体の doctest は `#[doc = include_str!(...)]` 経由で外部クレート利用の
//! 文脈でコンパイルされるため `fandhe_core::` を使うが、AI が実際に生成する画面ファイルは
//! クレート自身のモジュール内（本ファイルの配置）に置かれ、`fandhe-core` は
//! `extern crate self as fandhe_core;` を宣言していないため `fandhe_core::` は解決できない。
//! `crate::` で参照する必要があることを、本ファイル自体のコンパイル成立で検証する。
//!
//! `examples/schema_example_fixture.rs`・`tests/schema_example_fixture.rs`（別クレートとして
//! コンパイルされ、公開 API しか参照できない）から実際に利用可能なことを固定化するため、
//! 画面状態・ハンドラ・画面を組み立てる関数はすべて `pub` にする（PR #668 レビュー指摘）。
//!
//! ファイル名・モジュール名は SCHEMA.md 本文中の画面例（`todo`）とは意図的に変えている
//! （PR #668 レビュー対応）。AI が SCHEMA.md の例をそのまま画面名 `todo` として生成し
//! `pub mod todo;` を追加した場合に、このテスト専用モジュールと二重定義になって
//! コンパイル不能になるのを避けるため（`screens/mod.rs` は生成対象と同じ名前空間を共有する）。
//! 中身（`TodoScreenState` 等の識別子）は SCHEMA.md の例文そのままとし、
//! 固定化対象は識別子ではなく「配置先での `crate::` 参照がコンパイルできること」に絞る。

use crate::{Screen, WidgetId};

/// SCHEMA.md 例の画面状態（タスク一覧）。
#[derive(Default)]
pub struct TodoScreenState {
    /// 追加されたタスクの一覧。
    pub items: Vec<String>,
}

/// 「追加」ボタンのハンドラ（`Node::Button::on_click`）。
pub fn add_todo(state: &mut TodoScreenState) {
    state.items.push(format!("todo-{}", state.items.len() + 1));
}

/// SCHEMA.md 例の画面を組み立てる。`examples/`・`tests/` から利用する公開エントリポイント。
pub fn todo_screen() -> Screen<TodoScreenState> {
    Screen::new("タスク一覧")
        .text("タスク一覧")
        .button(WidgetId::from_raw(1), "追加", add_todo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{click_button, render_headless};

    #[test]
    fn todo_screen_click_adds_item() {
        let screen = todo_screen();
        let mut state = TodoScreenState::default();

        assert!(click_button(&screen, "追加", &mut state));
        assert_eq!(state.items, vec!["todo-1".to_string()]);
        assert!(!render_headless(&screen, &state).is_empty());
    }
}
