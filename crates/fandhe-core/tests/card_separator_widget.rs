//! `Card<S>` / `Separator<S>`（カード・セパレータウィジェット）の結合テスト
//! （TASK-8.3-S60 / REQ-8、Issue #155）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」）の一部。
//! `crates/fandhe-core/src/ui/card.rs` は `Widget<S>` のみに依存する自己完結モジュール
//! （`card.rs` モジュール doc 参照。公開面は Issue #153 に完全一致させてある）。
//!
//! disabled 状態・イベント処理（タップ・hover エレベーション等）の結合テストは
//! 本ファイルの対象外（Issue #154 のスコープ）。本ファイルは `tooltip_widget.rs`
//! が確立した 4 軸構成を踏襲し、公開 API のみを用いて検証する:
//!
//! 1. `WidgetHarness` 契約（S5 / #100）: `.card()` / `.separator()` を含む画面でも
//!    フレームループのバージョン再利用・初回ライフサイクル順（Declared → Mounted →
//!    Layout → Paint）が壊れないこと。
//! 2. `render_headless` プレースホルダ契約: `Node::Widget`（Card / Separator）は
//!    未配線のまま安全側（無出力）であること。
//! 3. `Widget` トレイト直接呼び出し（クレート外部・公開 API 経由の契約固定化。
//!    `card.rs` 内のユニットテストと重複する軸は代表ケースに絞り、外部から見える
//!    契約の固定化を主眼にする。Bugbot 回帰（`measure` / `paint` の内側幅一致）は
//!    `Horizontal` な `Separator` を子に持つ `Card` で検証する）。
//! 4. カタログ経由の到達性: `WidgetCatalog::with_builtins()` から `find("card")` /
//!    `find("separator")` でエントリが取得でき、`category` が期待どおりで、preview が
//!    panic せずタイトル行を返すこと。

use fandhe_core::{
    AccessibilityRole, Card, CatalogCategory, LifecyclePhase, Measurement, Node, PaintCommand,
    Screen, Separator, SeparatorOrientation, Widget, WidgetCatalog, WidgetHarness, WidgetId,
    render_headless,
};

/// 画面の状態。カード・セパレータはいずれも状態遷移を伴わないため（`tooltip_widget.rs`
/// の `FormState` と同じ判断）状態フィールドは持たない。
#[derive(Default)]
struct FormState;

fn sized(width: f32, height: f32) -> Measurement {
    Measurement { width, height }
}

fn card_separator_screen() -> Screen<FormState> {
    let card = Card::new(WidgetId::from_raw(1))
        .with_padding(4.0)
        .with_child(Separator::<FormState>::new(WidgetId::from_raw(2)));
    Screen::new("フォーム画面")
        .card(card)
        .separator(Separator::new(WidgetId::from_raw(3)))
}

// --- (1) WidgetHarness 契約 -------------------------------------------------

/// (1) `.card()` / `.separator()` を含む `Screen` を `WidgetHarness` で駆動しても、
/// S5 契約（バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_card_separator_screen() {
    let mut harness = WidgetHarness::new(card_separator_screen(), FormState);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (1) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`tooltip_widget.rs` と同じ検証軸を
/// `Card` / `Separator` を含む画面に対して確認する）。
#[test]
fn card_separator_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(card_separator_screen(), FormState);
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

/// (2) `render_headless` スナップショットに `Node::Widget`（`Card` / `Separator`）の
/// 行が現れないことを固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。
#[test]
fn render_headless_snapshot_does_not_contain_card_or_separator_line() {
    let screen = card_separator_screen();

    let snapshot = render_headless(&screen, &FormState);

    // タイトル行のみで、カード・セパレータの行はいずれも出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.card()` / `.separator()` 混在時も他バリアント（`.text()` 等）の描画・走査に
/// 影響しないことを確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の
/// 回帰防止）。
#[test]
fn card_separator_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面")
        .text("見出し")
        .card(Card::new(WidgetId::from_raw(1)))
        .separator(Separator::new(WidgetId::from_raw(2)));

    let snapshot = render_headless(&screen, &FormState);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// `Node::Widget` 経由で組み立てたカード・セパレータが 2 ノード分正しく追加される
/// ことを確認する（`tooltip_widget.rs` の `tooltip_screen_builds_one_widget_node` と
/// 同じ観点）。
#[test]
fn card_separator_screen_builds_two_widget_nodes() {
    let screen = card_separator_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 2);
}

// --- (3) Widget トレイト直接呼び出し（公開 API 経由の契約固定化） -----------

/// `Card::padding` が構築時に非有限値・負値を `0.0` へ丸めることを、クレート外部
/// （公開 API のみ）から固定化する。
#[test]
fn card_with_padding_clamps_negative_and_non_finite_to_zero_via_public_api() {
    let card: Card<FormState> = Card::new(WidgetId::from_raw(1)).with_padding(-3.0);
    assert_eq!(card.padding(), 0.0);

    let card: Card<FormState> = Card::new(WidgetId::from_raw(2)).with_padding(f32::NAN);
    assert_eq!(card.padding(), 0.0);
}

/// `Card::children_len` が `with_child` の呼び出し回数どおりであることを固定化する。
#[test]
fn card_children_len_reflects_with_child_calls_via_public_api() {
    let card: Card<FormState> = Card::new(WidgetId::from_raw(1))
        .with_child(Separator::new(WidgetId::from_raw(2)))
        .with_child(Separator::new(WidgetId::from_raw(3)));
    assert_eq!(card.children_len(), 2);
}

/// `Card::measure` と `Card::paint` が子へ渡す内側の利用可能領域（`available` /
/// 割り当て済み `size` から `padding * 2.0` を差し引いた領域）が一致することを、
/// 水平方向に伸縮する実際の子（`Separator`、`Horizontal` は `available.width` を
/// そのまま報告する）を用いて外部から固定化する（Bugbot 指摘の回帰、PR #397
/// レビュー・`card.rs` モジュール doc 参照）。
#[test]
fn card_measure_and_paint_agree_on_flexible_separator_child_width_via_public_api() {
    let card: Card<FormState> = Card::new(WidgetId::from_raw(1))
        .with_padding(10.0)
        .with_child(Separator::new(WidgetId::from_raw(2)));

    let available = sized(100.0, 100.0);
    let measured = card.measure(available);
    // 内側の利用可能領域 = 100 - padding*2(20) = 80。
    // カード全体の幅 = 80 + padding*2(20) = 100（Separator の水平方向は available.width
    // をそのまま報告するため、外側の available.width と一致する）。
    assert_eq!(measured.width, 100.0);

    // `measure` が返した寸法をそのままカードへ割り当てて `paint` した際も、
    // 子（Separator）が同じ内側幅（80.0）で再測定されることを、paint コマンド数
    // （子の Rect が 1 件、余分な生成がないこと）で間接的に固定化する。
    let commands = card.paint((0.0, 0.0), measured);
    assert_eq!(commands, vec![PaintCommand::Rect, PaintCommand::Rect]);
}

/// `Card::hit_test` が `disabled` の間は常に `false` を返すことを固定化する。
#[test]
fn card_hit_test_returns_false_when_disabled_via_public_api() {
    let mut card: Card<FormState> = Card::new(WidgetId::from_raw(1));
    card.set_disabled(true);
    assert!(!card.hit_test((5.0, 5.0), (0.0, 0.0), sized(10.0, 10.0)));
}

/// `Card::accessibility_role()` が [`AccessibilityRole::Group`] を返すことを固定化する
/// （`stack_grid::Stack` / `stack_grid::Grid` と共用するロール、`card.rs` 参照）。
#[test]
fn card_accessibility_role_is_group_via_public_api() {
    let card: Card<FormState> = Card::new(WidgetId::from_raw(1));
    assert_eq!(card.accessibility_role(), AccessibilityRole::Group);
}

/// `Separator::new` の既定値（水平・太さ 1.0）を、クレート外部（公開 API のみ）から
/// 固定化する。
#[test]
fn separator_new_defaults_to_horizontal_with_thickness_one_via_public_api() {
    let separator: Separator<FormState> = Separator::new(WidgetId::from_raw(1));
    assert_eq!(separator.orientation(), SeparatorOrientation::Horizontal);
    assert_eq!(separator.thickness(), 1.0);
}

/// `Separator::with_thickness` が非有限値・非正値を既定値へフォールバックすることを
/// 固定化する（見えない区切り線を許容しない安全側判断、`card.rs` 参照）。
#[test]
fn separator_with_thickness_falls_back_to_default_on_non_positive_via_public_api() {
    let separator: Separator<FormState> =
        Separator::new(WidgetId::from_raw(1)).with_thickness(-2.0);
    assert_eq!(separator.thickness(), 1.0);

    let separator: Separator<FormState> = Separator::new(WidgetId::from_raw(2)).with_thickness(0.0);
    assert_eq!(separator.thickness(), 1.0);
}

/// `Separator::measure` が `Horizontal` / `Vertical` それぞれで `available` の対応する
/// 成分と `thickness` を組み合わせて返すことを固定化する。
#[test]
fn separator_measure_uses_available_and_thickness_via_public_api() {
    let horizontal: Separator<FormState> =
        Separator::new(WidgetId::from_raw(1)).with_thickness(2.0);
    let measured = horizontal.measure(sized(80.0, 999.0));
    assert_eq!(measured.width, 80.0);
    assert_eq!(measured.height, 2.0);

    let vertical: Separator<FormState> = Separator::new(WidgetId::from_raw(2))
        .with_orientation(SeparatorOrientation::Vertical)
        .with_thickness(3.0);
    let measured = vertical.measure(sized(999.0, 60.0));
    assert_eq!(measured.width, 3.0);
    assert_eq!(measured.height, 60.0);
}

/// `Separator::hit_test` が常に `false` を返すことを固定化する（装飾専用、
/// `card.rs` の `Separator::hit_test` doc 参照）。
#[test]
fn separator_hit_test_is_always_false_via_public_api() {
    let separator: Separator<FormState> = Separator::new(WidgetId::from_raw(1));
    assert!(!separator.hit_test((5.0, 0.0), (0.0, 0.0), sized(80.0, 1.0)));
}

/// `Separator::accessibility_role()` が [`AccessibilityRole::Separator`] を返すことを
/// 固定化する（`WidgetCatalog` 側の回帰テストが依拠する専用ロール、`catalog.rs` の
/// `separator_preview_widget_is_backed_by_real_separator` 参照）。
#[test]
fn separator_accessibility_role_is_separator_via_public_api() {
    let separator: Separator<FormState> = Separator::new(WidgetId::from_raw(1));
    assert_eq!(separator.accessibility_role(), AccessibilityRole::Separator);
}

// --- (4) カタログ経由の到達性 ------------------------------------------------

/// `WidgetCatalog::with_builtins()` から `card` エントリが取得でき、
/// `category == Container`・preview が panic せずタイトル行を返すことを確認する
/// （TASK-8.3-S60 / Issue #155、カタログ登録の受け入れ条件）。
#[test]
fn card_entry_is_registered_in_container_category_with_non_empty_preview() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("card")
        .expect("card エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Container);

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# Card プレビュー".to_string()]);

    let container_entries = catalog.entries_in(CatalogCategory::Container);
    assert!(container_entries.iter().any(|e| e.name == "card"));
}

/// `WidgetCatalog::with_builtins()` から `separator` エントリが取得でき、
/// `category == Display`・preview が panic せずタイトル行を返すことを確認する
/// （TASK-8.3-S60 / Issue #155、カタログ登録の受け入れ条件）。
#[test]
fn separator_entry_is_registered_in_display_category_with_non_empty_preview() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("separator")
        .expect("separator エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Display);

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# Separator プレビュー".to_string()]);

    let display_entries = catalog.entries_in(CatalogCategory::Display);
    assert!(display_entries.iter().any(|e| e.name == "separator"));
}
