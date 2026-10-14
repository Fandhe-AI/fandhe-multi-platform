//! `Menu<S>`（メニュー・ドロップダウンウィジェット）の結合テスト（TASK-8.3-S51 / REQ-8、
//! Issue #146）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」）の一部。
//! `crates/fandhe-core/src/ui/catalog.rs` の `menu` エントリは `Menu<S>`
//! （`menu.rs` 参照）へ `Screen::menu` 経由で接続する（`catalog.rs` の
//! `with_builtins` / `menu_preview_screen` / `preview_menu` doc 参照）。
//!
//! 開閉・選択イベント配線（トリガークリック・項目クリック・外側クリック閉じ・
//! キーボード操作）は `Menu` の `apply_pointer` / `handle_input` が担う（Issue #145）。
//! 本ファイルの (5) に、`StateStore::dispatch` を介したトリガークリック → open 反映 →
//! 項目クリック → 選択 + close 反映のエンドツーエンドシナリオを含む（宣言層はレガシー
//! `.button(label, handler)` ノードで受ける、`checkbox` の検証パターン踏襲）。
//!
//! 本ファイルは `Menu::bound` / `Widget<S>` トレイト直接呼び出し・
//! `WidgetHarness`・`WidgetCatalog` の 3 経路から公開 API のみを用いて検証する:
//!
//! 1. `WidgetHarness` 契約（S5 / #100）: `.menu()` を含む画面でもフレームループの
//!    バージョン再利用・初回ライフサイクル順（Declared → Mounted → Layout → Paint）が
//!    壊れないこと。
//! 2. `render_headless` プレースホルダ契約: `Node::Widget` は未配線のまま安全側
//!    （無出力）であること。
//! 3. `Widget` トレイト直接呼び出し（クレート外部・公開 API 経由の契約固定化。
//!    `menu.rs` 内のユニットテスト（約 20 件）と重複する軸は代表ケースに絞り、
//!    外部から見える契約の固定化を主眼にする）。
//! 4. カタログ経由の到達性: `WidgetCatalog::with_builtins()` から `find("menu")` で
//!    エントリが取得でき、`category == Overlay`・preview が panic せずタイトル行を
//!    返すこと。

use fandhe_core::{
    AccessibilityRole, CatalogCategory, InteractionInput, LifecyclePhase, Measurement, Menu,
    MenuHitRegion, Node, PaintCommand, Screen, StateStore, Widget, WidgetCatalog, WidgetHarness,
    WidgetId, render_headless,
};

/// 画面の状態。`menu_open` / `selected` フィールドをメニューウィジェットが読み取る想定。
#[derive(Default)]
struct FormState {
    menu_open: bool,
    selected: Option<usize>,
}

fn get_menu_open(state: &FormState) -> bool {
    state.menu_open
}

fn get_selected(state: &FormState) -> Option<usize> {
    state.selected
}

fn menu_items() -> Vec<String> {
    vec!["項目 A".to_string(), "項目 B".to_string()]
}

fn menu_screen() -> Screen<FormState> {
    Screen::new("フォーム画面").menu(
        WidgetId::from_raw(1),
        "選択してください",
        menu_items(),
        get_menu_open,
        get_selected,
    )
}

fn sized(width: f32, height: f32) -> Measurement {
    Measurement { width, height }
}

// --- (1) WidgetHarness 契約 -------------------------------------------------

/// (1) `.menu()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_menu_screen() {
    let mut harness = WidgetHarness::new(menu_screen(), FormState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (1) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`dialog_widget.rs` と同じ検証軸を
/// `Menu` を含む画面に対して確認する）。
#[test]
fn menu_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(menu_screen(), FormState::default());
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

/// (2) `render_headless` スナップショットに `Node::Widget`（`Menu`）の行が現れないことを
/// 固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。`sync_from_state`
/// （`&mut self` 要求）を呼べる経路が `render_headless` に存在しないため、`paint()` への
/// 委譲は行わず安全側（無出力）に倒す契約を、メニューについても回帰検知できるように
/// する。
#[test]
fn render_headless_snapshot_does_not_contain_menu_line() {
    let screen = menu_screen();
    let state = FormState {
        menu_open: true,
        selected: None,
    };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、メニューの行（トリガー・項目いずれも）は出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.menu()` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn menu_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面").text("見出し").menu(
        WidgetId::from_raw(1),
        "選択してください",
        menu_items(),
        get_menu_open,
        get_selected,
    );
    let state = FormState {
        menu_open: false,
        selected: None,
    };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// `Node::Widget` 経由で組み立てたメニューが 1 ノード分正しく追加されることを確認する
/// （`dialog_widget.rs` の `dialog_screen_builds_one_widget_node` と同じ観点）。
#[test]
fn menu_screen_builds_one_widget_node() {
    let screen = menu_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}

// --- (3) Widget トレイト直接呼び出し（公開 API 経由の契約固定化） -----------

/// 閉状態（既定）はトリガー矩形のみを対象とすることを、クレート外部（公開 API のみ）
/// から固定化する（`Dialog` の「閉状態はゼロ寸法」とは異なり、`Menu` は閉状態でも
/// トリガーボタンが常に存在する UI のため、`measure` はゼロ寸法を返さない、
/// `menu.rs` モジュール doc 参照）。
#[test]
fn closed_menu_is_measured_and_painted_as_trigger_only() {
    let menu: Menu<FormState> = Menu::bound(
        WidgetId::from_raw(1),
        "選択してください",
        menu_items(),
        get_menu_open,
        get_selected,
    );

    let measurement = menu.measure(sized(800.0, 600.0));
    assert!(measurement.width > 0.0);
    assert!(measurement.height > 0.0);

    let commands = menu.paint((0.0, 0.0), measurement);
    assert_eq!(
        commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Text("選択してください".to_string()),
        ]
    );

    assert!(menu.hit_test((10.0, 10.0), (0.0, 0.0), measurement));
}

/// 開状態はトリガー + ポップアップの `PaintCommand` 列を返し、`hit_test` が `measure`
/// の返す確定寸法全域をカバーすることを固定化する（`Dialog` が PR #386 Bugbot 指摘で
/// 修正した整合性契約と同型、`menu.rs` モジュール doc 参照）。
#[test]
fn open_menu_paints_trigger_and_popup_and_hit_test_covers_measured_size() {
    let mut menu: Menu<FormState> = Menu::bound(
        WidgetId::from_raw(1),
        "選択してください",
        menu_items(),
        get_menu_open,
        get_selected,
    );
    menu.sync_from_state(&FormState {
        menu_open: true,
        selected: None,
    });

    let available = sized(800.0, 600.0);
    let measured = menu.measure(available);

    let commands = menu.paint((0.0, 0.0), measured);
    assert_eq!(
        commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Text("選択してください".to_string()),
            PaintCommand::Rect,
            PaintCommand::Text("項目 A".to_string()),
            PaintCommand::Text("項目 B".to_string()),
        ]
    );

    assert!(menu.hit_test((0.0, 0.0), (0.0, 0.0), measured));
    assert!(menu.hit_test(
        (measured.width - 1.0, measured.height - 1.0),
        (0.0, 0.0),
        measured
    ));
    assert!(!menu.hit_test((measured.width, measured.height), (0.0, 0.0), measured));
}

/// `sync_from_state` による開 → 閉 → 開の遷移を、公開 API 経由で固定化する。
#[test]
fn sync_from_state_transitions_open_closed_open_via_public_api() {
    let mut menu: Menu<FormState> = Menu::bound(
        WidgetId::from_raw(1),
        "選択してください",
        menu_items(),
        get_menu_open,
        get_selected,
    );

    let mut state = FormState {
        menu_open: true,
        selected: None,
    };
    menu.sync_from_state(&state);
    assert!(menu.is_open());

    state.menu_open = false;
    menu.sync_from_state(&state);
    assert!(!menu.is_open());

    state.menu_open = true;
    menu.sync_from_state(&state);
    assert!(menu.is_open());
}

/// 選択中項目 index がトリガー表示テキストへ反映されることを、公開 API 経由で
/// 固定化する（`menu.rs` の `selected_index_reflects_in_displayed_trigger_text` と
/// 同じ観点をクレート外部から再確認する）。
#[test]
fn selected_index_reflects_in_displayed_trigger_text_via_public_api() {
    let mut menu: Menu<FormState> = Menu::bound(
        WidgetId::from_raw(1),
        "選択してください",
        menu_items(),
        get_menu_open,
        get_selected,
    );
    menu.sync_from_state(&FormState {
        menu_open: false,
        selected: Some(1),
    });

    assert_eq!(menu.selected_index(), Some(1));
    let commands = menu.paint((0.0, 0.0), menu.measure(sized(800.0, 600.0)));
    assert_eq!(commands[1], PaintCommand::Text("項目 B".to_string()));
}

/// `accessibility_role()` が [`AccessibilityRole::ComboBox`] を返すことを固定化する
/// （`WidgetCatalog` 側の回帰テストが依拠する専用ロール、`catalog.rs` の
/// `menu_preview_widget_is_backed_by_real_menu` 参照）。
#[test]
fn accessibility_role_is_combo_box() {
    let menu: Menu<FormState> = Menu::bound(
        WidgetId::from_raw(1),
        "選択してください",
        menu_items(),
        get_menu_open,
        get_selected,
    );
    assert_eq!(menu.accessibility_role(), AccessibilityRole::ComboBox);
}

// --- (4) カタログ経由の到達性 ------------------------------------------------

/// `WidgetCatalog::with_builtins()` から `menu` エントリが取得でき、
/// `category == Overlay`・preview が panic せずタイトル行を返すことを確認する
/// （TASK-8.3-S51 / Issue #146、カタログ登録の受け入れ条件）。
#[test]
fn menu_entry_is_registered_in_overlay_category_with_non_empty_preview() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("menu")
        .expect("menu エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Overlay);

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# Menu プレビュー".to_string()]);

    let overlay_entries = catalog.entries_in(CatalogCategory::Overlay);
    assert!(overlay_entries.iter().any(|e| e.name == "menu"));
}

// --- (5) 開閉・選択イベント配線（TASK-8.3-S50 / Issue #145、StateStore::dispatch 経由） ---

/// トリガークリック → open 反映 → 項目クリック → 選択 + close 反映という一連の
/// エンドツーエンドシナリオを検証する（宣言層はレガシー `.button(label, handler)` で
/// 受け、`StateStore::dispatch` を経由して状態へ反映後、`sync_from_state` で新値が
/// 還流することを確認する。`dialog_widget.rs` の
/// `state_store_dispatch_closes_dialog_via_legacy_button_bridge` と同型の検証パターン）。
#[test]
fn trigger_click_then_item_click_opens_selects_and_closes_via_state_store_dispatch() {
    fn toggle_menu(state: &mut FormState) {
        state.menu_open = !state.menu_open;
    }

    fn select_item_a(state: &mut FormState) {
        state.selected = Some(0);
        state.menu_open = false;
    }

    let screen: Screen<FormState> = Screen::new("フォーム画面")
        .button(WidgetId::from_raw(1), "選択してください", toggle_menu)
        .button(WidgetId::from_raw(2), "項目 A", select_item_a);
    let mut store = StateStore::new(FormState::default());
    let mut menu: Menu<FormState> = Menu::bound(
        WidgetId::from_raw(1),
        "選択してください",
        menu_items(),
        get_menu_open,
        get_selected,
    );
    menu.sync_from_state(store.state());
    assert!(!menu.is_open());

    // (1) トリガークリック → StateStore::dispatch → open 反映。
    menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Trigger);
    let trigger_event = menu
        .apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Trigger)
        .expect("トリガー press/release は Some を返す");
    assert!(store.dispatch(&screen, &trigger_event));
    menu.sync_from_state(store.state());
    assert!(menu.is_open());
    assert_eq!(menu.focused_item(), Some(0));

    // (2) 項目クリック（項目 A、index 0）→ StateStore::dispatch → 選択 + close 反映。
    menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Item(0));
    let select_event = menu
        .apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Item(0))
        .expect("同一項目上の press/release は Some を返す");
    assert!(store.dispatch(&screen, &select_event));
    menu.sync_from_state(store.state());

    assert!(!menu.is_open());
    assert_eq!(menu.selected_index(), Some(0));
    assert_eq!(store.state().selected, Some(0));
    assert!(!store.state().menu_open);
}
