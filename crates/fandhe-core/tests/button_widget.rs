//! `Button<S>`（ボタンウィジェット）の結合テスト（TASK-8.3-S9 / REQ-8、Issue #104）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」）の一部。
//! `crates/fandhe-core/src/ui/catalog.rs` の `button` エントリの接続先はレガシー
//! `Node::Button`（`Screen::button` ビルダー）ではなく `Button<S>`（`button.rs` 参照、
//! Issue #102、#103）（`catalog.rs` の `builtin_entries` / `button_preview_screen` /
//! `preview_button` doc 参照）。
//!
//! `tabs_widget.rs`・`tooltip_widget.rs` が確立した 4 軸構成を踏襲しつつ、
//! 以下 3 の「イベント処理」軸に押下確定・キーボード活性化・`StateStore::dispatch` への
//! ブリッジまでを含める（`button.rs` 内ユニットテストと重複する軸は代表ケースに絞り、
//! クレート外部・公開 API 経由での契約固定化を主眼にする）:
//!
//! 1. `WidgetHarness` 契約（S5 / #100）: `Node::Widget(Button)` を含む画面でもフレーム
//!    ループのバージョン再利用・初回ライフサイクル順（Declared → Mounted → Layout → Paint）
//!    が壊れないこと。
//! 2. `render_headless` プレースホルダ契約: `Node::Widget` は未配線のまま安全側
//!    （無出力）であること。
//! 3. `Widget` トレイト直接呼び出し + イベント処理（クレート外部・公開 API 経由の契約
//!    固定化）: measure / paint / hit_test / accessibility_role・押下確定（ポインタ・
//!    キーボード）・`StateStore::dispatch` へのブリッジ。
//! 4. カタログ経由の到達性: `WidgetCatalog::with_builtins()` から `find("button")` で
//!    エントリが取得でき、`category == Action`・preview が panic せずタイトル行を返すこと。

use fandhe_core::{
    AccessibilityRole, CatalogCategory, Event, InteractionInput, LifecyclePhase, Measurement, Node,
    PaintCommand, Screen, StateStore, Widget, WidgetCatalog, WidgetHarness, WidgetId,
    render_headless,
};
use fandhe_core::{Button, ButtonInput};

/// 画面の状態。`count` フィールドをボタン押下ハンドラが更新する想定
/// （`button.rs` 内ユニットテストの `CounterState` と同型）。
#[derive(Default)]
struct CounterState {
    count: i32,
}

fn increment(state: &mut CounterState) {
    state.count += 1;
}

fn button_screen() -> Screen<CounterState> {
    Screen::new("フォーム画面").with_node(Node::Widget(Box::new(Button::new(
        WidgetId::from_raw(1),
        "送信",
        increment,
    ))))
}

fn sized(width: f32, height: f32) -> Measurement {
    Measurement { width, height }
}

// --- (1) WidgetHarness 契約 -------------------------------------------------

/// (1) `Node::Widget(Button)` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_button_screen() {
    let mut harness = WidgetHarness::new(button_screen(), CounterState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (1) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`tabs_widget.rs` と同じ検証軸を
/// `Button` を含む画面に対して確認する）。
#[test]
fn button_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(button_screen(), CounterState::default());
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

/// (2) `render_headless` スナップショットに `Node::Widget`（`Button`）の行が現れないことを
/// 固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。`sync_from_state`
/// （`&mut self` 要求）を呼べる経路が `render_headless` に存在しないため、`paint()` への
/// 委譲は行わず安全側（無出力）に倒す契約を、ボタンについても回帰検知できるようにする。
#[test]
fn render_headless_snapshot_does_not_contain_button_line() {
    let screen = button_screen();
    let state = CounterState { count: 0 };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、ボタンの行（背景・ラベルいずれも）は出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `Node::Widget(Button)` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn button_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<CounterState> =
        Screen::new("混在画面")
            .text("見出し")
            .with_node(Node::Widget(Box::new(Button::new(
                WidgetId::from_raw(1),
                "送信",
                increment,
            ))));
    let state = CounterState { count: 0 };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// `Node::Widget` 経由で組み立てたボタンが 1 ノード分正しく追加されることを確認する
/// （`tabs_widget.rs` の `tabs_screen_builds_one_widget_node` と同じ観点）。
#[test]
fn button_screen_builds_one_widget_node() {
    let screen = button_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}

// --- (3) Widget トレイト直接呼び出し + イベント処理（公開 API 経由の契約固定化） ---

/// `measure` が文字数近似からラベル寸法を導出し、利用可能領域へクランプすることを、
/// クレート外部（公開 API のみ）から固定化する。
#[test]
fn measure_approximates_label_and_clamps_to_available_space() {
    let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

    let generous = button.measure(sized(1000.0, 1000.0));
    assert!(generous.width > 0.0);
    assert!(generous.height > 0.0);

    let clamped = button.measure(sized(5.0, 5.0));
    assert_eq!(clamped.width, 5.0);
    assert_eq!(clamped.height, 5.0);
}

/// `paint` が `[Rect（背景）, Text(label)]` の 2 コマンドを返すことを、公開 API 経由で
/// 固定化する（`tabs_widget.rs` の `selected_tab_is_reflected_in_painted_labels_via_public_api`
/// と同じ観点）。
#[test]
fn paint_returns_background_rect_and_label_text_via_public_api() {
    let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

    let commands = button.paint((0.0, 0.0), button.measure(sized(800.0, 600.0)));
    assert_eq!(
        commands,
        vec![PaintCommand::Rect, PaintCommand::Text("送信".to_string())]
    );
}

/// `hit_test` が `measure` の返す確定寸法全域をカバーし、disabled 中は常に `false` を
/// 返すことを、クレート外部から固定化する（`tabs_widget.rs` の `hit_test_covers_measured_size`
/// と同じ観点に disabled 分岐を追加）。
#[test]
fn hit_test_covers_measured_size_and_respects_disabled() {
    let mut button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

    let available = sized(800.0, 600.0);
    let measured = button.measure(available);

    assert!(button.hit_test((0.0, 0.0), (0.0, 0.0), measured));
    assert!(button.hit_test(
        (measured.width - 1.0, measured.height - 1.0),
        (0.0, 0.0),
        measured
    ));
    assert!(!button.hit_test((measured.width, measured.height), (0.0, 0.0), measured));

    button.set_disabled(true);
    assert!(!button.hit_test((0.0, 0.0), (0.0, 0.0), measured));
}

/// `accessibility_role()` が [`AccessibilityRole::Button`] を返すことを固定化する
/// （`WidgetCatalog` 側の回帰テストが依拠するロール、`catalog.rs` の
/// `button_preview_widget_is_backed_by_real_button` 参照）。
#[test]
fn accessibility_role_is_button() {
    let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);
    assert_eq!(button.accessibility_role(), AccessibilityRole::Button);
}

/// ポインタ押下 → 領域内解放で `Event::ButtonClicked` が発火することを、クレート外部から
/// 固定化する（S8 のポインタ確定契約、`button.rs` 内ユニットテストと同じ観点をクレート
/// 外部の公開 API のみで再確認する）。
#[test]
fn pointer_press_and_release_inside_confirms_click_via_public_api() {
    let mut button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

    button.apply_input(InteractionInput::PointerEntered);
    button.apply_input(InteractionInput::PointerPressed);
    let event = button.apply_input(InteractionInput::PointerReleased);

    assert_eq!(
        event,
        Some(Event::ButtonClicked {
            label: "送信".to_string(),
        })
    );
}

/// 押下 → 領域外へ exit → 解放では発火しないことを、クレート外部から固定化する
/// （press-and-drag-away の意味論、`button.rs` の
/// `pointer_press_and_drag_away_does_not_confirm_click` と同じ観点）。
#[test]
fn pointer_press_and_drag_away_does_not_confirm_click_via_public_api() {
    let mut button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

    button.apply_input(InteractionInput::PointerEntered);
    button.apply_input(InteractionInput::PointerPressed);
    button.apply_input(InteractionInput::PointerExited);
    let event = button.apply_input(InteractionInput::PointerReleased);

    assert_eq!(event, None);
}

/// フォーカス中の `ButtonInput::Activate` で `Event::ButtonClicked` が発火し、非フォーカス
/// 中・disabled 中は無視されることを、クレート外部から固定化する。
#[test]
fn keyboard_activate_confirms_only_when_focused_and_enabled() {
    let mut focused: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);
    focused.apply_input(InteractionInput::FocusGained);
    let event = focused.handle_input(ButtonInput::Activate);
    assert_eq!(
        event,
        Some(Event::ButtonClicked {
            label: "送信".to_string(),
        })
    );

    let mut not_focused: Button<CounterState> =
        Button::new(WidgetId::from_raw(2), "送信", increment);
    assert_eq!(not_focused.handle_input(ButtonInput::Activate), None);

    let mut disabled: Button<CounterState> = Button::new(WidgetId::from_raw(3), "送信", increment);
    disabled.apply_input(InteractionInput::FocusGained);
    disabled.set_disabled(true);
    assert_eq!(disabled.handle_input(ButtonInput::Activate), None);
}

/// 発火した `Event::ButtonClicked` を `StateStore::dispatch` へ渡すと、宣言層の
/// レガシー `.button(label, handler)` ブリッジ（`event::click_button`）経由で状態が
/// 更新されることを固定化する（S8 のブリッジ設計のエンドツーエンド検証、`button.rs` の
/// `state_store_dispatch_increments_state_via_legacy_button_bridge` と同じ観点をクレート
/// 外部の公開 API のみで再確認する）。
#[test]
fn dispatching_button_clicked_event_increments_state_via_legacy_bridge() {
    let screen: Screen<CounterState> =
        Screen::new("form").button(WidgetId::from_raw(1), "送信", increment);
    let mut store = StateStore::new(CounterState { count: 0 });
    let mut button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);
    button.apply_input(InteractionInput::FocusGained);

    let event = button
        .handle_input(ButtonInput::Activate)
        .expect("フォーカス中の Activate は Some を返す");
    let handled = store.dispatch(&screen, &event);

    assert!(handled);
    assert_eq!(store.state().count, 1);
    assert_eq!(store.version(), 1);
}

// --- (4) カタログ経由の到達性 ------------------------------------------------

/// `WidgetCatalog::with_builtins()` から `button` エントリが取得でき、
/// `category == Action`・preview が panic せずタイトル行を返すことを確認する
/// （TASK-8.3-S9 / Issue #104、カタログ登録の受け入れ条件）。
#[test]
fn button_entry_is_registered_in_action_category_with_non_empty_preview() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("button")
        .expect("button エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Action);

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# Button プレビュー".to_string()]);

    let action_entries = catalog.entries_in(CatalogCategory::Action);
    assert!(action_entries.iter().any(|e| e.name == "button"));
    assert_eq!(
        action_entries.len(),
        1,
        "Action カテゴリは button エントリのみで構成される（S9 時点、catalog.rs 参照）"
    );
}
