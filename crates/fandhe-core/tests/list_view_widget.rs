//! `ListView`（TASK-8.3-S16 / REQ-8 / REQ-13、Issue #111）の結合テスト。
//!
//! 親 issue #95 の一部。`tests/label_widget.rs`・`tests/button_widget.rs` の構成を
//! 踏襲し、`Screen::list_view` ビルダーを含む画面を `WidgetHarness` で駆動して、
//! (a) フレームループ・バージョン再利用の意味論が `Node::Widget` 追加によって
//! 壊れないこと、(b) `render_headless` スナップショットにウィジェット行が現れない
//! （`widget.rs` の未配線契約の固定化）ことを検証する。加えて `ListView` を直接駆動
//! する「宣言 → sync_from_state → measure → paint」のライフサイクル順テスト
//! （状態変更を反映することを含む）と、`WidgetCatalog` 経由の到達性テスト
//! （Issue #113）を置く。ユニットレベルの `measure` / `paint` / `hit_test` /
//! `accessibility_role` / `sync_from_state` 網羅は
//! `crates/fandhe-core/src/ui/list_view.rs` の `#[cfg(test)] mod tests` 側に
//! まとめている（`Label` / `Button` と同じ分担方針）。選択状態・`hit_test` 差し替え等
//! は本ファイルの対象外（Issue #112 のスコープ、`list_view.rs` モジュール doc 参照）。

use fandhe_core::{
    CatalogCategory, ListView, Measurement, Node, Screen, Widget, WidgetCatalog, WidgetHarness,
    WidgetId, render_headless,
};

/// 項目一覧を保持する状態。`Screen::list_view` の束縛先として使う。
#[derive(Default)]
struct TodoList {
    items: Vec<String>,
}

fn read_len(state: &TodoList) -> usize {
    state.items.len()
}

fn read_item(state: &TodoList, index: usize) -> String {
    state.items[index].clone()
}

fn add_item(state: &mut TodoList) {
    state.items.push(format!("item-{}", state.items.len()));
}

fn list_screen() -> Screen<TodoList> {
    Screen::new("home")
        .text("見出し")
        .list_view(WidgetId::from_raw(1), read_len, read_item)
        .button(WidgetId::from_raw(2), "add", add_item)
}

/// `.list_view()` を含む画面でもフレームループ・バージョン再利用の意味論
/// （`WidgetHarness` 共通契約、TASK-8.3-S5）が壊れないことを確認する回帰テスト。
#[test]
fn list_view_screen_does_not_break_harness_frame_loop_semantics() {
    let mut harness = WidgetHarness::new(list_screen(), TodoList::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);

    // 状態バージョンが変化していない 2 回目の `run_frame` はスナップショットを
    // 再構築しない（`ui::ListRecycler` と同じ再利用条件）。
    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// `Node::Widget` は `render_headless` の状態非依存プレースホルダである
/// （`widget.rs` の `Widget::sync_from_state` doc に明記された未配線契約）。本テストは
/// この契約を固定化し、`paint()` への委譲復活を回帰として検知する。
///
/// `label_widget.rs` と異なり、`ListView` は `paint()` が空リストなら空コマンド列を
/// 返すため、`sync_from_state` を経ていない（キャッシュが空の）状態のまま検証すると
/// `paint()` への委譲が復活しても出力が変わらず、回帰を検知できない
/// （Cursor Bugbot 指摘 f3f6d4a1-a385-4529-a118-3373d45fbec7）。そのため本テストは
/// `Screen::list_view` ビルダー（キャッシュ未同期）を使わず、`ListView::bound` を
/// 直接 `sync_from_state` で同期して項目を非空にしたうえで `Node::Widget` として
/// 組み込み、`render_headless` を直接呼び出す。これにより `paint()` への委譲が
/// 復活すれば `PaintCommand::Text` 相当の行が追加され、完全一致が崩れて検知できる。
#[test]
fn render_headless_snapshot_does_not_include_list_view_widget_output() {
    let mut list: ListView<TodoList> = ListView::bound(WidgetId::from_raw(1), read_len, read_item);
    let state = TodoList {
        items: vec!["a".to_string(), "b".to_string()],
    };
    // sync_from_state でキャッシュを非空にしておく（`paint()` が空リストゆえに
    // 空コマンド列を返すケースを排除し、委譲復活を検知可能にするための前提）。
    list.sync_from_state(&state);
    assert!(!list.items().is_empty());

    let screen: Screen<TodoList> = Screen::new("home")
        .text("見出し")
        .with_node(Node::Widget(Box::new(list)))
        .button(WidgetId::from_raw(2), "add", add_item);
    let snapshot = render_headless(&screen, &state);

    // `ListView` の行は現れない（`Node::Widget` は no-op）ため、`Text` 見出し・
    // `Button` の行のみで完全一致する。将来 `render_headless` が `Node::Widget` を
    // `paint()` へ委譲するよう変更されれば、キャッシュ済み項目 "a" / "b" の行が
    // 追加されこの完全一致は崩れる。
    assert_eq!(
        snapshot,
        vec![
            "# home".to_string(),
            "見出し".to_string(),
            "(add)".to_string()
        ]
    );
}

/// `ListView` を直接駆動する「宣言 → sync_from_state → measure → paint」の順序テスト。
/// `WidgetHarness` を経由しない、ウィジェット実体単体でのライフサイクル検証。状態変更が
/// 反映されることも合わせて確認する。
#[test]
fn list_view_direct_drive_sync_measure_paint_lifecycle() {
    let mut list: ListView<TodoList> = ListView::bound(WidgetId::from_raw(2), read_len, read_item);

    // 宣言直後（sync 前）は空。
    assert!(list.items().is_empty());

    // sync_from_state で状態からキャッシュを更新（reconcile フェーズ）。
    list.sync_from_state(&TodoList {
        items: vec!["a".to_string(), "b".to_string()],
    });
    assert_eq!(list.items(), ["a".to_string(), "b".to_string()]);

    // measure はキャッシュ済み項目列の文字数・件数に基づく（レイアウトフェーズ）。
    let measurement = list.measure(Measurement {
        width: 1000.0,
        height: 1000.0,
    });
    assert!(measurement.width > 0.0);
    assert!(measurement.height > 0.0);

    // paint はキャッシュ済み項目列をそのまま描画コマンドへ変換する（描画フェーズ）。
    use fandhe_core::PaintCommand;
    let commands = list.paint((0.0, 0.0), measurement);
    assert_eq!(
        commands,
        vec![
            PaintCommand::Text("a".to_string()),
            PaintCommand::Text("b".to_string())
        ]
    );

    // 状態変更が sync_from_state で反映されることを確認する。
    list.sync_from_state(&TodoList {
        items: vec!["c".to_string()],
    });
    assert_eq!(list.items(), ["c".to_string()]);
}

/// `WidgetCatalog::with_builtins()` から `list` エントリが取得でき、
/// `category == Container`・preview が panic せずタイトル行を返すことを確認する
/// （TASK-8.3-S18 / Issue #113、カタログ登録の受け入れ条件、`button_widget.rs` の
/// `button_entry_is_registered_in_action_category_with_non_empty_preview` と同型）。
#[test]
fn list_entry_is_registered_in_container_category_with_non_empty_preview() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("list")
        .expect("list エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Container);

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# List プレビュー".to_string()]);

    let container_entries = catalog.entries_in(CatalogCategory::Container);
    assert!(container_entries.iter().any(|e| e.name == "list"));
}
