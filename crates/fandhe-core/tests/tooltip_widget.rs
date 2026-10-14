//! `Tooltip<S>`（ツールチップウィジェット）の結合テスト（TASK-8.3-S57 / REQ-8、
//! Issue #152）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」）の一部。
//! `crates/fandhe-core/src/ui/catalog.rs` の `tooltip` エントリは `Tooltip<S>`
//! （`tooltip.rs` 参照）へ `Screen::tooltip` 経由で接続する（`catalog.rs` の
//! `with_builtins` / `tooltip_preview_screen` / `preview_tooltip` doc 参照）。
//!
//! hover/focus 起因の表示・非表示イベント配線の結合テストは本ファイルの対象外
//! （Issue #151 のスコープ）。本ファイルは `Tooltip::fixed` / `Widget<S>` トレイト
//! 直接呼び出し・`WidgetHarness`・`WidgetCatalog` の 4 経路から公開 API のみを用いて
//! 検証する（`tabs_widget.rs` が確立した 4 軸構成を踏襲する）:
//!
//! 1. `WidgetHarness` 契約（S5 / #100）: `.tooltip()` を含む画面でもフレームループの
//!    バージョン再利用・初回ライフサイクル順（Declared → Mounted → Layout → Paint）が
//!    壊れないこと。
//! 2. `render_headless` プレースホルダ契約: `Node::Widget` は未配線のまま安全側
//!    （無出力）であること。
//! 3. `Widget` トレイト直接呼び出し（クレート外部・公開 API 経由の契約固定化。
//!    `tooltip.rs` 内のユニットテストと重複する軸は代表ケースに絞り、外部から見える
//!    契約の固定化を主眼にする）。
//! 4. カタログ経由の到達性: `WidgetCatalog::with_builtins()` から `find("tooltip")` で
//!    エントリが取得でき、`category == Overlay`・preview が panic せずタイトル行を
//!    返すこと。

use fandhe_core::{
    AccessibilityRole, CatalogCategory, Label, LifecyclePhase, Measurement, Node, PaintCommand,
    Screen, Tooltip, Widget, WidgetCatalog, WidgetHarness, WidgetId, render_headless,
};

/// 画面の状態。ツールチップの本文は固定テキストのため状態フィールドは持たない
/// （`Tooltip::fixed` を利用する、`tooltip.rs` の `Tooltip::fixed` doc 参照）。
#[derive(Default)]
struct FormState;

fn make_anchor() -> Label<FormState> {
    Label::fixed(WidgetId::from_raw(2), "保存")
}

fn tooltip_screen() -> Screen<FormState> {
    Screen::new("フォーム画面").tooltip(WidgetId::from_raw(1), "保存します", make_anchor())
}

fn sized(width: f32, height: f32) -> Measurement {
    Measurement { width, height }
}

// --- (1) WidgetHarness 契約 -------------------------------------------------

/// (1) `.tooltip()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_tooltip_screen() {
    let mut harness = WidgetHarness::new(tooltip_screen(), FormState);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (1) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`tabs_widget.rs` と同じ検証軸を
/// `Tooltip` を含む画面に対して確認する）。
#[test]
fn tooltip_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(tooltip_screen(), FormState);
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

/// (2) `render_headless` スナップショットに `Node::Widget`（`Tooltip`）の行が
/// 現れないことを固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。
/// `sync_from_state`（`&mut self` 要求）を呼べる経路が `render_headless` に存在しない
/// ため、`paint()` への委譲は行わず安全側（無出力）に倒す契約を、ツールチップに
/// ついても回帰検知できるようにする。
#[test]
fn render_headless_snapshot_does_not_contain_tooltip_line() {
    let screen = tooltip_screen();

    let snapshot = render_headless(&screen, &FormState);

    // タイトル行のみで、ツールチップの行（アンカー・バブルいずれも）は出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.tooltip()` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn tooltip_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面").text("見出し").tooltip(
        WidgetId::from_raw(1),
        "保存します",
        make_anchor(),
    );

    let snapshot = render_headless(&screen, &FormState);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// `Node::Widget` 経由で組み立てたツールチップが 1 ノード分正しく追加されることを
/// 確認する（`tabs_widget.rs` の `tabs_screen_builds_one_widget_node` と同じ観点）。
#[test]
fn tooltip_screen_builds_one_widget_node() {
    let screen = tooltip_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}

// --- (3) Widget トレイト直接呼び出し（公開 API 経由の契約固定化） -----------

/// `text()` が構築時の固定本文をそのまま返すことを、クレート外部（公開 API のみ）
/// から固定化する（`Tooltip::fixed` は構築直後から `sync_from_state` 前でも本文が
/// 反映される契約、`tooltip.rs` の `Tooltip::fixed` doc 参照）。
#[test]
fn fixed_returns_constructed_text_without_sync() {
    let tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(1), "保存します");
    assert_eq!(tooltip.text(), "保存します");
    assert!(!tooltip.is_visible());
}

/// `show()` 後の `paint` にアンカーの描画コマンドへ続けて `[Rect, Text(本文)]` が
/// 連結されることを、公開 API 経由で固定化する（`tooltip.rs` の
/// `show_appends_bubble_after_anchor_paint` と同じ観点をクレート外部から再確認する）。
#[test]
fn show_appends_bubble_after_anchor_paint_via_public_api() {
    let mut tooltip: Tooltip<FormState> =
        Tooltip::fixed(WidgetId::from_raw(1), "保存します").with_anchor(make_anchor());

    tooltip.show();
    let commands = tooltip.paint((0.0, 0.0), sized(80.0, 20.0));

    assert_eq!(commands.len(), 3);
    assert_eq!(commands[1], PaintCommand::Rect);
    assert_eq!(commands[2], PaintCommand::Text("保存します".to_string()));
}

/// `hide()` / 既定の非表示状態ではバブルが `paint` に一切含まれないことを固定化する。
#[test]
fn hidden_paint_has_no_bubble_via_public_api() {
    let tooltip: Tooltip<FormState> =
        Tooltip::fixed(WidgetId::from_raw(1), "保存します").with_anchor(make_anchor());

    let commands = tooltip.paint((0.0, 0.0), sized(80.0, 20.0));

    assert!(!commands.contains(&PaintCommand::Rect));
}

/// `set_disabled(true)` の場合は `show()` 済みでもバブルが抑止されることを固定化する。
#[test]
fn disabled_suppresses_bubble_even_when_shown_via_public_api() {
    let mut tooltip: Tooltip<FormState> =
        Tooltip::fixed(WidgetId::from_raw(1), "保存します").with_anchor(make_anchor());
    tooltip.show();
    tooltip.set_disabled(true);

    let commands = tooltip.paint((0.0, 0.0), sized(80.0, 20.0));

    assert!(!commands.contains(&PaintCommand::Rect));
}

/// `measure` がアンカーの計測結果をそのまま返すことを固定化する（アンカーがある場合、
/// `tooltip.rs` の `measure_delegates_to_anchor` と同じ観点をクレート外部から
/// 再確認する）。
#[test]
fn measure_delegates_to_anchor_via_public_api() {
    let tooltip: Tooltip<FormState> =
        Tooltip::fixed(WidgetId::from_raw(1), "保存します").with_anchor(make_anchor());

    let measurement = tooltip.measure(sized(100.0, 40.0));
    let expected = make_anchor().measure(sized(100.0, 40.0));

    assert_eq!(measurement.width, expected.width);
    assert_eq!(measurement.height, expected.height);
}

/// アンカーなしの `measure` はゼロ寸法を返すことを固定化する。
#[test]
fn measure_without_anchor_is_zero_via_public_api() {
    let tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(1), "保存します");

    let measurement = tooltip.measure(sized(100.0, 40.0));

    assert_eq!(measurement.width, 0.0);
    assert_eq!(measurement.height, 0.0);
}

/// `hit_test` がアンカーへ委譲することを固定化する（バブル自体は非インタラクティブ、
/// `tooltip.rs` の `hit_test_delegates_to_anchor` と同じ観点）。
#[test]
fn hit_test_delegates_to_anchor_via_public_api() {
    let tooltip: Tooltip<FormState> =
        Tooltip::fixed(WidgetId::from_raw(1), "保存します").with_anchor(make_anchor());
    let size = sized(80.0, 20.0);

    let expected = make_anchor().hit_test((5.0, 5.0), (0.0, 0.0), size);
    let actual = tooltip.hit_test((5.0, 5.0), (0.0, 0.0), size);

    assert_eq!(actual, expected);
}

/// アンカーなしの `hit_test` は常に `false` を返すことを固定化する。
#[test]
fn hit_test_without_anchor_is_false_via_public_api() {
    let tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(1), "保存します");

    assert!(!tooltip.hit_test((5.0, 5.0), (0.0, 0.0), sized(80.0, 20.0)));
}

/// `accessibility_role()` が [`AccessibilityRole::Tooltip`] を返すことを固定化する
/// （`WidgetCatalog` 側の回帰テストが依拠する専用ロール、`catalog.rs` の
/// `tooltip_preview_widget_is_backed_by_real_tooltip` 参照）。
#[test]
fn accessibility_role_is_tooltip() {
    let tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(1), "保存します");
    assert_eq!(tooltip.accessibility_role(), AccessibilityRole::Tooltip);
}

// --- (4) カタログ経由の到達性 ------------------------------------------------

/// `WidgetCatalog::with_builtins()` から `tooltip` エントリが取得でき、
/// `category == Overlay`・preview が panic せずタイトル行を返すことを確認する
/// （TASK-8.3-S57 / Issue #152、カタログ登録の受け入れ条件）。
#[test]
fn tooltip_entry_is_registered_in_overlay_category_with_non_empty_preview() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("tooltip")
        .expect("tooltip エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Overlay);

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# Tooltip プレビュー".to_string()]);

    let overlay_entries = catalog.entries_in(CatalogCategory::Overlay);
    assert!(overlay_entries.iter().any(|e| e.name == "tooltip"));
}
