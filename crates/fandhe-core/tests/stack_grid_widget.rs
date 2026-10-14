//! `stack` / `grid` ウィジェットカタログエントリの結合テスト（TASK-8.3-S45 / REQ-8、
//! Issue #140）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」）の一部。
//! `crates/fandhe-core/src/ui/catalog.rs` の `stack` / `grid` エントリは `Stack<S>` /
//! `Grid<S>`（`stack_grid.rs` 参照）へ `Screen::stack` / `Screen::grid` 経由で接続する
//! （`catalog.rs` の `with_builtins` / `preview_stack` / `preview_grid` doc 参照）。
//!
//! 本ファイルは公開 API（[`WidgetCatalog`]）のみを使い、(1)(2)(5) でカタログ登録・
//! 分類・preview 契約を検証し、(3)(4) で `render_headless` の `Node::Widget`
//! プレースホルダ契約（`widget.rs` 参照。子の実描画は reconcile 配線後のスコープの
//! ため出力はタイトル行のみ）を固定化する。`preview_stack` / `preview_grid` が
//! 実際に `Stack<S>` / `Grid<S>` の子を保持していること自体の固定化（クレート内部の
//! `Node::Widget` を `Screen::visit` で取り出し `Widget::measure` / `accessibility_role`
//! で検証する回帰テスト）は `catalog.rs` の
//! `stack_preview_widget_is_backed_by_real_stack_with_two_children` /
//! `grid_preview_widget_is_backed_by_real_grid_with_children`（`#[cfg(test)] mod tests`）
//! で行う（本ファイルは crate 外部のためクレート非公開の `Node<S>` 走査ができない）。
//! `WidgetHarness` フレームループ検証（インタラクション状態の遷移）は本ファイルの
//! 対象外（Issue #139 のスコープ）。

use fandhe_core::render_headless;
use fandhe_core::ui::{CatalogCategory, WidgetCatalog};

/// (1) `stack` / `grid` エントリが組み込みカタログへ登録され、`Container` カテゴリで
/// 一意に検索できることを確認する（`catalog.rs` のユニットテストと同じ観点を、
/// クレート外部（`tests/`）から公開 API のみで固定化する）。
#[test]
fn stack_and_grid_entries_are_registered_in_container_category() {
    let catalog = WidgetCatalog::with_builtins();

    let stack_entry = catalog
        .find("stack")
        .expect("stack エントリが登録されているべき");
    assert_eq!(stack_entry.category, CatalogCategory::Container);

    let grid_entry = catalog
        .find("grid")
        .expect("grid エントリが登録されているべき");
    assert_eq!(grid_entry.category, CatalogCategory::Container);

    let container_entries = catalog.entries_in(CatalogCategory::Container);
    assert!(container_entries.iter().any(|e| e.name == "stack"));
    assert!(container_entries.iter().any(|e| e.name == "grid"));
}

/// (2) `stack` / `grid` の preview が空でない出力を返すことを確認する（S7 以降が倣う
/// preview 契約の最小確認、`with_builtins_registers_*` 系と対の検証）。
#[test]
fn stack_and_grid_previews_produce_non_empty_output() {
    let catalog = WidgetCatalog::with_builtins();

    let stack_entry = catalog.find("stack").expect("stack エントリが必要");
    let stack_lines = (stack_entry.preview)();
    assert!(!stack_lines.is_empty());
    assert_eq!(stack_lines[0], "# Stack プレビュー");

    let grid_entry = catalog.find("grid").expect("grid エントリが必要");
    let grid_lines = (grid_entry.preview)();
    assert!(!grid_lines.is_empty());
    assert_eq!(grid_lines[0], "# Grid プレビュー");
}

/// (3) `stack` の preview の `render_headless` 出力がタイトル行のみであることを
/// 固定化する回帰テスト。`Screen::stack` は `Node::Widget` 経由で `Stack<S>` を
/// 保持するが、`render_headless` は `Node::Widget` を未配線のプレースホルダとして
/// 扱う契約（`widget.rs` 参照）のため、子ラベルの文字列は出力に現れない
/// （`scroll_view` エントリと同型の契約、`preview_scroll_view` 参照）。
#[test]
fn stack_preview_render_headless_output_is_title_only() {
    let catalog = WidgetCatalog::with_builtins();
    let entry = catalog.find("stack").expect("stack エントリが必要");

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# Stack プレビュー".to_string()]);
}

/// (4) `grid` の preview も (3) と同じ `render_headless` プレースホルダ契約に従う
/// ことを確認する（(3) と対の回帰テスト）。
#[test]
fn grid_preview_render_headless_output_is_title_only() {
    let catalog = WidgetCatalog::with_builtins();
    let entry = catalog.find("grid").expect("grid エントリが必要");

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# Grid プレビュー".to_string()]);
}

/// (5) `stack` / `grid` エントリの追加が既存エントリのギャラリー表示に影響しないこと
/// （既存ウィジェットとの回帰防止）。ギャラリー画面をヘッドレス描画し、既存の
/// `scroll_view` 行と新規の `stack` / `grid` 行が両方とも含まれることを確認する。
#[test]
fn gallery_render_includes_existing_and_new_container_entries() {
    let catalog = WidgetCatalog::with_builtins();
    let state = catalog.gallery_state();
    let screen = catalog.gallery_screen();

    let rendered = render_headless(&screen, &state);

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("scroll_view（コンテナ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("stack（コンテナ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("grid（コンテナ）"))
    );
}
