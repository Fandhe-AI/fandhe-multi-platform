//! `Dialog<S>`（ダイアログ・モーダルウィジェット）の結合テスト（TASK-8.3-S48 / REQ-8、
//! Issue #143）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」）の一部。
//! `crates/fandhe-core/src/ui/catalog.rs` の `dialog` エントリは `Dialog<S>`
//! （`dialog.rs` 参照）へ `Screen::dialog` 経由で接続する（`catalog.rs` の
//! `with_builtins` / `dialog_preview_screen` / `preview_dialog` doc 参照）。
//!
//! 開閉イベント配線（背景クリック・Esc・閉じるボタンによる書き込み）・フォーカス
//! トラップの結合テストは本ファイルの対象外（Issue #142 のスコープ）。本ファイルは
//! `Dialog::bound` / `Widget<S>` トレイト直接呼び出し・`WidgetHarness`・
//! `WidgetCatalog` の 3 経路から公開 API のみを用いて検証する:
//!
//! 1. `WidgetHarness` 契約（S5 / #100）: `.dialog()` を含む画面でもフレームループの
//!    バージョン再利用・初回ライフサイクル順（Declared → Mounted → Layout → Paint）が
//!    壊れないこと。
//! 2. `render_headless` プレースホルダ契約: `Node::Widget` は未配線のまま安全側
//!    （無出力）であること。
//! 3. `Widget` トレイト直接呼び出し（クレート外部・公開 API 経由の契約固定化。
//!    `dialog.rs` 内のユニットテスト（約 30 件）と重複する軸は代表ケースに絞り、
//!    外部から見える契約の固定化を主眼にする）。
//! 4. カタログ経由の到達性: `WidgetCatalog::with_builtins()` から `find("dialog")` で
//!    エントリが取得でき、`category == Overlay`・preview が panic せずタイトル行を
//!    返すこと。

use fandhe_core::{
    AccessibilityRole, CatalogCategory, Dialog, LifecyclePhase, Measurement, Node, PaintCommand,
    Screen, Widget, WidgetCatalog, WidgetHarness, WidgetId, render_headless,
};

/// フォーム画面の状態。`dialog_open` フィールドをダイアログウィジェットが読み取る想定。
#[derive(Default)]
struct FormState {
    dialog_open: bool,
}

fn get_dialog_open(state: &FormState) -> bool {
    state.dialog_open
}

fn dialog_screen() -> Screen<FormState> {
    Screen::new("フォーム画面").dialog(
        WidgetId::from_raw(1),
        "確認",
        "保存しますか？",
        get_dialog_open,
    )
}

fn sized(width: f32, height: f32) -> Measurement {
    Measurement { width, height }
}

// --- (1) WidgetHarness 契約 -------------------------------------------------

/// (1) `.dialog()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_dialog_screen() {
    let mut harness = WidgetHarness::new(dialog_screen(), FormState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (1) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`icon_widget.rs` と同じ検証軸を
/// `Dialog` を含む画面に対して確認する）。
#[test]
fn dialog_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(dialog_screen(), FormState::default());
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

/// (2) `render_headless` スナップショットに `Node::Widget`（`Dialog`）の行が現れないことを
/// 固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。`sync_from_state`
/// （`&mut self` 要求）を呼べる経路が `render_headless` に存在しないため、`paint()` への
/// 委譲は行わず安全側（無出力）に倒す契約を、ダイアログについても回帰検知できるように
/// する。
#[test]
fn render_headless_snapshot_does_not_contain_dialog_line() {
    let screen = dialog_screen();
    let state = FormState { dialog_open: true };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、ダイアログの行（タイトル・本文いずれも）は出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.dialog()` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn dialog_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面").text("見出し").dialog(
        WidgetId::from_raw(1),
        "確認",
        "本文",
        get_dialog_open,
    );
    let state = FormState { dialog_open: false };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// `Node::Widget` 経由で組み立てたダイアログが 1 ノード分正しく追加されることを確認する
/// （`icon_widget.rs` の `icon_screen_builds_one_widget_node` と同じ観点）。
#[test]
fn dialog_screen_builds_one_widget_node() {
    let screen = dialog_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}

// --- (3) Widget トレイト直接呼び出し（公開 API 経由の契約固定化） -----------

/// 閉状態（既定）はゼロ寸法・空 paint・hit_test false であることを、クレート外部
/// （公開 API のみ）から固定化する。
#[test]
fn closed_dialog_is_measured_and_painted_as_absent() {
    let dialog: Dialog<FormState> = Dialog::bound(
        WidgetId::from_raw(1),
        "確認",
        "保存しますか？",
        get_dialog_open,
    );

    let measurement = dialog.measure(sized(800.0, 600.0));
    assert_eq!(measurement.width, 0.0);
    assert_eq!(measurement.height, 0.0);

    let commands = dialog.paint((0.0, 0.0), sized(800.0, 600.0));
    assert!(commands.is_empty());

    assert!(!dialog.hit_test((10.0, 10.0), (0.0, 0.0), sized(800.0, 600.0)));
}

/// 開状態（modal）は scrim → パネル → タイトル → 本文の `PaintCommand` 列を返し、
/// `hit_test` が `measure` の返す確定寸法全域（scrim を含む）をカバーすることを
/// 固定化する。
#[test]
fn open_modal_dialog_paints_full_sequence_and_hit_test_covers_measured_size() {
    let mut dialog: Dialog<FormState> = Dialog::bound(
        WidgetId::from_raw(1),
        "確認",
        "保存しますか？",
        get_dialog_open,
    );
    dialog.sync_from_state(&FormState { dialog_open: true });

    let available = sized(800.0, 600.0);
    let measured = dialog.measure(available);

    let commands = dialog.paint((0.0, 0.0), measured);
    assert_eq!(
        commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Rect,
            PaintCommand::Text("確認".to_string()),
            PaintCommand::Text("保存しますか？".to_string()),
        ]
    );

    assert!(dialog.hit_test((0.0, 0.0), (0.0, 0.0), measured));
    assert!(dialog.hit_test((799.0, 599.0), (0.0, 0.0), measured));
    assert!(!dialog.hit_test((800.0, 600.0), (0.0, 0.0), measured));
}

/// 開状態（非 modal）は scrim を含まない `PaintCommand` 列を返し、`hit_test` が
/// パネル矩形に限定されることを固定化する（`measured` は `available` より小さい）。
#[test]
fn open_non_modal_dialog_paints_without_scrim_and_hit_test_is_limited_to_panel() {
    let mut dialog: Dialog<FormState> = Dialog::bound(
        WidgetId::from_raw(1),
        "確認",
        "保存しますか？",
        get_dialog_open,
    )
    .with_modal(false);
    dialog.sync_from_state(&FormState { dialog_open: true });

    let available = sized(800.0, 600.0);
    let measured = dialog.measure(available);
    assert!(measured.width < available.width);
    assert!(measured.height < available.height);

    let commands = dialog.paint((0.0, 0.0), measured);
    assert_eq!(
        commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Text("確認".to_string()),
            PaintCommand::Text("保存しますか？".to_string()),
        ]
    );

    assert!(dialog.hit_test((1.0, 1.0), (0.0, 0.0), measured));
    assert!(!dialog.hit_test((799.0, 599.0), (0.0, 0.0), measured));
}

/// `sync_from_state` による開 → 閉 → 開の遷移を、公開 API 経由で固定化する。
#[test]
fn sync_from_state_transitions_open_closed_open_via_public_api() {
    let mut dialog: Dialog<FormState> = Dialog::bound(
        WidgetId::from_raw(1),
        "確認",
        "保存しますか？",
        get_dialog_open,
    );

    let mut state = FormState { dialog_open: true };
    dialog.sync_from_state(&state);
    assert!(dialog.is_open());

    state.dialog_open = false;
    dialog.sync_from_state(&state);
    assert!(!dialog.is_open());

    state.dialog_open = true;
    dialog.sync_from_state(&state);
    assert!(dialog.is_open());
}

/// 子コンテンツ（`with_content`）の paint 連結・`sync_from_state` の委譲を、
/// `Label::fixed`（子側の状態非依存な固定表示）を使って固定化する。
#[test]
fn with_content_appends_child_paint_when_open() {
    use fandhe_core::Label;

    let mut dialog: Dialog<FormState> = Dialog::bound(
        WidgetId::from_raw(1),
        "確認",
        "保存しますか？",
        get_dialog_open,
    )
    .with_content(Label::fixed(WidgetId::from_raw(2), "補足情報"));
    dialog.sync_from_state(&FormState { dialog_open: true });

    let commands = dialog.paint((0.0, 0.0), sized(800.0, 600.0));

    assert_eq!(commands.len(), 5);
    assert_eq!(commands[4], PaintCommand::Text("補足情報".to_string()));
}

/// `accessibility_role()` が [`AccessibilityRole::Dialog`] を返すことを固定化する
/// （`WidgetCatalog` 側の回帰テストが依拠する専用ロール、`catalog.rs` の
/// `dialog_preview_widget_is_backed_by_real_dialog` 参照）。
#[test]
fn accessibility_role_is_dialog() {
    let dialog: Dialog<FormState> = Dialog::bound(
        WidgetId::from_raw(1),
        "確認",
        "保存しますか？",
        get_dialog_open,
    );
    assert_eq!(dialog.accessibility_role(), AccessibilityRole::Dialog);
}

// --- (4) カタログ経由の到達性 ------------------------------------------------

/// `WidgetCatalog::with_builtins()` から `dialog` エントリが取得でき、
/// `category == Overlay`・preview が panic せずタイトル行を返すことを確認する
/// （TASK-8.3-S48 / Issue #143、カタログ登録の受け入れ条件）。
#[test]
fn dialog_entry_is_registered_in_overlay_category_with_non_empty_preview() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("dialog")
        .expect("dialog エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Overlay);

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# Dialog プレビュー".to_string()]);

    let overlay_entries = catalog.entries_in(CatalogCategory::Overlay);
    assert!(overlay_entries.iter().any(|e| e.name == "dialog"));
}
