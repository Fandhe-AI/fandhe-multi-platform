//! `Tabs<S>`（タブウィジェット）の結合テスト（TASK-8.3-S54 / REQ-8、Issue #149）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」）の一部。
//! `crates/fandhe-core/src/ui/catalog.rs` の `tabs` エントリは `Tabs<S>`
//! （`tabs.rs` 参照）へ `Screen::tabs` 経由で接続する（`catalog.rs` の
//! `with_builtins` / `tabs_preview_screen` / `preview_tabs` doc 参照）。
//!
//! タブ切替イベント配線（クリック・キーボード操作の `WriteFn` 書き込み）の結合テストは
//! 本ファイルの対象外（Issue #148 のスコープ）。本ファイルは `Tabs::bound` / `Widget<S>`
//! トレイト直接呼び出し・`WidgetHarness`・`WidgetCatalog` の 4 経路から公開 API のみを
//! 用いて検証する（`menu_widget.rs` が確立した 4 軸構成を踏襲する）:
//!
//! 1. `WidgetHarness` 契約（S5 / #100）: `.tabs()` を含む画面でもフレームループの
//!    バージョン再利用・初回ライフサイクル順（Declared → Mounted → Layout → Paint）が
//!    壊れないこと。
//! 2. `render_headless` プレースホルダ契約: `Node::Widget` は未配線のまま安全側
//!    （無出力）であること。
//! 3. `Widget` トレイト直接呼び出し（クレート外部・公開 API 経由の契約固定化。
//!    `tabs.rs` 内のユニットテストと重複する軸は代表ケースに絞り、外部から見える契約の
//!    固定化を主眼にする）。
//! 4. カタログ経由の到達性: `WidgetCatalog::with_builtins()` から `find("tabs")` で
//!    エントリが取得でき、`category == Overlay`・preview が panic せずタイトル行を
//!    返すこと。

use fandhe_core::{
    AccessibilityRole, CatalogCategory, LifecyclePhase, Measurement, Node, PaintCommand, Screen,
    Tabs, Widget, WidgetCatalog, WidgetHarness, WidgetId, render_headless,
};

/// 画面の状態。`selected_tab` フィールドをタブウィジェットが読み取る想定。
#[derive(Default)]
struct FormState {
    selected_tab: usize,
}

fn get_selected_tab(state: &FormState) -> usize {
    state.selected_tab
}

fn tab_labels() -> Vec<String> {
    vec!["概要".to_string(), "詳細".to_string()]
}

fn tabs_screen() -> Screen<FormState> {
    Screen::new("フォーム画面").tabs(WidgetId::from_raw(1), tab_labels(), get_selected_tab)
}

fn sized(width: f32, height: f32) -> Measurement {
    Measurement { width, height }
}

// --- (1) WidgetHarness 契約 -------------------------------------------------

/// (1) `.tabs()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_tabs_screen() {
    let mut harness = WidgetHarness::new(tabs_screen(), FormState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (1) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`menu_widget.rs` と同じ検証軸を
/// `Tabs` を含む画面に対して確認する）。
#[test]
fn tabs_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(tabs_screen(), FormState::default());
    harness.run_frame();

    assert_eq!(
        harness.phases(),
        &[
            LifecyclePhase::Declared,
            LifecyclePhase::Mounted,
            LifecyclePhase::Layout,
            LifecyclePhase::Paint
        ]
    );
}

// --- (2) render_headless プレースホルダ契約 ---------------------------------

/// (2) `render_headless` スナップショットに `Node::Widget`（`Tabs`）の行が現れないことを
/// 固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。`sync_from_state`
/// （`&mut self` 要求）を呼べる経路が `render_headless` に存在しないため、`paint()` への
/// 委譲は行わず安全側（無出力）に倒す契約を、タブについても回帰検知できるようにする。
#[test]
fn render_headless_snapshot_does_not_contain_tabs_line() {
    let screen = tabs_screen();
    let state = FormState { selected_tab: 0 };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、タブの行（見出し・コンテンツいずれも）は出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.tabs()` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn tabs_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面").text("見出し").tabs(
        WidgetId::from_raw(1),
        tab_labels(),
        get_selected_tab,
    );
    let state = FormState { selected_tab: 0 };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// `Node::Widget` 経由で組み立てたタブが 1 ノード分正しく追加されることを確認する
/// （`menu_widget.rs` の `menu_screen_builds_one_widget_node` と同じ観点）。
#[test]
fn tabs_screen_builds_one_widget_node() {
    let screen = tabs_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}

// --- (3) Widget トレイト直接呼び出し（公開 API 経由の契約固定化） -----------

/// `labels()` が構築時の見出し一覧をそのまま返すことを、クレート外部（公開 API のみ）
/// から固定化する。
#[test]
fn labels_returns_constructed_headings() {
    let tabs: Tabs<FormState> = Tabs::bound(WidgetId::from_raw(1), tab_labels(), get_selected_tab);
    assert_eq!(tabs.labels(), vec!["概要", "詳細"]);
}

/// `sync_from_state` による選択タブの遷移を、公開 API 経由で固定化する
/// （`menu_widget.rs` の `sync_from_state_transitions_open_closed_open_via_public_api`
/// と同じ観点）。
#[test]
fn sync_from_state_transitions_selected_tab_via_public_api() {
    let mut tabs: Tabs<FormState> =
        Tabs::bound(WidgetId::from_raw(1), tab_labels(), get_selected_tab);

    let mut state = FormState { selected_tab: 0 };
    tabs.sync_from_state(&state);
    assert_eq!(tabs.selected_index(), Some(0));

    state.selected_tab = 1;
    tabs.sync_from_state(&state);
    assert_eq!(tabs.selected_index(), Some(1));
}

/// 選択中タブの装飾表記付き表示テキストが `paint` の出力に反映されることを、
/// 公開 API 経由で固定化する（`tabs.rs` の `paint_produces_bar_rect_labels_and_content_area_rect`
/// と同じ観点をクレート外部から再確認する）。
#[test]
fn selected_tab_is_reflected_in_painted_labels_via_public_api() {
    let mut tabs: Tabs<FormState> =
        Tabs::bound(WidgetId::from_raw(1), tab_labels(), get_selected_tab);
    tabs.sync_from_state(&FormState { selected_tab: 1 });

    let commands = tabs.paint((0.0, 0.0), tabs.measure(sized(800.0, 600.0)));
    assert_eq!(
        commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Text("概要".to_string()),
            PaintCommand::Text("[詳細]".to_string()),
            PaintCommand::Rect,
        ]
    );
}

/// `hit_test` が `measure` の返す確定寸法全域をカバーすることを、クレート外部から
/// 固定化する（`Dialog` / `Menu` が PR #386 Bugbot 指摘で修正した整合性契約と同型、
/// `tabs.rs` モジュール doc 参照）。
#[test]
fn hit_test_covers_measured_size() {
    let mut tabs: Tabs<FormState> =
        Tabs::bound(WidgetId::from_raw(1), tab_labels(), get_selected_tab);
    tabs.sync_from_state(&FormState { selected_tab: 0 });

    let available = sized(800.0, 600.0);
    let measured = tabs.measure(available);

    assert!(tabs.hit_test((0.0, 0.0), (0.0, 0.0), measured));
    assert!(tabs.hit_test(
        (measured.width - 1.0, measured.height - 1.0),
        (0.0, 0.0),
        measured
    ));
    assert!(!tabs.hit_test((measured.width, measured.height), (0.0, 0.0), measured));
}

/// `accessibility_role()` が [`AccessibilityRole::TabList`] を返すことを固定化する
/// （`WidgetCatalog` 側の回帰テストが依拠する専用ロール、`catalog.rs` の
/// `tabs_preview_widget_is_backed_by_real_tabs` 参照）。
#[test]
fn accessibility_role_is_tab_list() {
    let tabs: Tabs<FormState> = Tabs::bound(WidgetId::from_raw(1), tab_labels(), get_selected_tab);
    assert_eq!(tabs.accessibility_role(), AccessibilityRole::TabList);
}

// --- (4) カタログ経由の到達性 ------------------------------------------------

/// `WidgetCatalog::with_builtins()` から `tabs` エントリが取得でき、
/// `category == Overlay`・preview が panic せずタイトル行を返すことを確認する
/// （TASK-8.3-S54 / Issue #149、カタログ登録の受け入れ条件）。
#[test]
fn tabs_entry_is_registered_in_overlay_category_with_non_empty_preview() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("tabs")
        .expect("tabs エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Overlay);

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# Tabs プレビュー".to_string()]);

    let overlay_entries = catalog.entries_in(CatalogCategory::Overlay);
    assert!(overlay_entries.iter().any(|e| e.name == "tabs"));
}
