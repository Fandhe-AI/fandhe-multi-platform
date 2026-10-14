//! `Label` / `LabelWidget`（TASK-8.3-S10 / S11 / S12、REQ-8 / REQ-13、
//! Issue #105 / #106 / #107）の結合テスト。
//!
//! 親 issue #95 の一部。`Screen::label` ビルダーを含む画面を `WidgetHarness` で
//! 駆動し、(a) フレームループ・バージョン再利用の意味論が `Node::Widget` 追加によって
//! 壊れないこと、(b) `render_headless` スナップショットにウィジェット行が現れない
//! （`widget.rs` の未配線契約の固定化）ことを検証する。加えて `Label` / `LabelWidget`
//! を直接駆動するライフサイクル・インタラクション状態の結合テストを、クレート外部
//! （公開 API 経由）で検証する（Issue #107）。

use fandhe_core::{
    AccessibilityRole, HitRegion, InteractionInput, Label, LabelWidget, Measurement, PaintCommand,
    Point, Rect, Screen, Size, Widget, WidgetHarness, WidgetId,
};

/// カウンタ状態。`Screen::label` の束縛先として使う。
#[derive(Default)]
struct Counter {
    count: u32,
}

fn increment(state: &mut Counter) {
    state.count += 1;
}

fn read_count(state: &Counter) -> String {
    format!("count: {}", state.count)
}

fn label_screen() -> Screen<Counter> {
    Screen::new("home")
        .text("見出し")
        .label(WidgetId::from_raw(1), read_count)
        .button(WidgetId::from_raw(2), "increment", increment)
}

/// `.label()` を含む画面でもフレームループ・バージョン再利用の意味論（`WidgetHarness`
/// 共通契約、TASK-8.3-S5）が壊れないことを確認する回帰テスト。
#[test]
fn label_screen_does_not_break_harness_frame_loop_semantics() {
    let mut harness = WidgetHarness::new(label_screen(), Counter::default());

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
/// `Screen::label` で束縛したラベルは `sync_from_state` が一度も呼ばれないため
/// `cached_text` は常に空文字列になる。そのため「`count:` を含む行が無い」という
/// 部分一致の否定だけでは、`render_headless` が将来 `paint()` へ委譲するよう
/// 変更されても空文字列の `PaintCommand::Text` が出力されるだけで検知できない
/// （空文字列の行が増えても `contains("count:")` は false のまま）。
/// 出力行数・内容を完全一致で固定化し、余分な行の出現自体を回帰として検知する。
#[test]
fn render_headless_snapshot_does_not_include_label_widget_output() {
    let mut harness = WidgetHarness::new(label_screen(), Counter::default());
    let snapshot = harness.run_frame().to_vec();

    // `Label` の行は現れない（`Node::Widget` は no-op）ため、`Text` 見出し・
    // `Button` の行のみで完全一致する。将来 `render_headless` が `Node::Widget` を
    // `paint()` へ委譲するよう変更されれば、空文字列であっても行が追加され
    // この完全一致は崩れる。
    assert_eq!(
        snapshot,
        vec![
            "# home".to_string(),
            "見出し".to_string(),
            "(increment)".to_string(),
        ]
    );
}

/// `Label` を直接駆動する「宣言 → sync_from_state → measure → paint」の順序テスト。
/// `WidgetHarness` を経由しない、ウィジェット実体単体でのライフサイクル検証。
#[test]
fn label_direct_drive_sync_measure_paint_lifecycle() {
    let mut label: Label<Counter> = Label::bound(WidgetId::from_raw(2), read_count);

    // 宣言直後（sync 前）は空文字列。
    assert_eq!(label.text(), "");

    // sync_from_state で状態からキャッシュを更新（reconcile フェーズ）。
    label.sync_from_state(&Counter { count: 7 });
    assert_eq!(label.text(), "count: 7");

    // measure はキャッシュ済みテキストの文字数に基づく（レイアウトフェーズ）。
    let measurement = label.measure(Measurement {
        width: 1000.0,
        height: 100.0,
    });
    assert!(measurement.width > 0.0);

    // paint はキャッシュ済みテキストをそのまま描画コマンドへ変換する（描画フェーズ）。
    let commands = label.paint((0.0, 0.0), measurement);
    assert_eq!(commands, vec![PaintCommand::Text("count: 7".to_string())]);
}

/// `Label::fixed`（S10）の直接駆動ライフサイクル。`sync_from_state` は状態に依存しない
/// 固定文言のため呼び出し前後で `text()` が変化しないこと（`label.rs` の
/// `LabelSource::Static` 分岐 doc 参照）、`measure` → `paint` が一貫して固定文言を
/// 参照することを確認する（TASK-8.3-S12 / Issue #107）。
#[test]
fn label_fixed_sync_from_state_is_no_op_and_lifecycle_stays_consistent() {
    let mut label: Label<Counter> = Label::fixed(WidgetId::from_raw(3), "固定ラベル");
    assert_eq!(label.text(), "固定ラベル");

    // 固定文言は状態に依存しないため、sync_from_state 呼び出し後も値は変わらない。
    label.sync_from_state(&Counter { count: 99 });
    assert_eq!(label.text(), "固定ラベル");

    let measurement = label.measure(Measurement {
        width: 1000.0,
        height: 100.0,
    });
    let commands = label.paint((0.0, 0.0), measurement);
    assert_eq!(commands, vec![PaintCommand::Text("固定ラベル".to_string())]);
}

/// `LabelWidget::new_static`（S11）の sync 挙動: `sync_from_state` は常に no-op で
/// あり、構築時の固定文言を保持し続けることを公開 API 経由で確認する
/// （TASK-8.3-S12 / Issue #107）。
#[test]
fn label_widget_new_static_sync_from_state_is_no_op() {
    let mut label: LabelWidget<Counter> =
        LabelWidget::new_static(WidgetId::from_raw(4), "静的テキスト");
    assert_eq!(label.text(), "静的テキスト");

    label.sync_from_state(&Counter { count: 5 });
    assert_eq!(label.text(), "静的テキスト");
}

/// `LabelWidget::new_dynamic`（S11）の sync 挙動: 構築直後は `initial` を保持し、
/// `sync_from_state` 呼び出しごとに状態変化へ追随することを公開 API 経由で確認する
/// （TASK-8.3-S12 / Issue #107）。
#[test]
fn label_widget_new_dynamic_sync_from_state_tracks_state_changes() {
    let mut label = LabelWidget::new_dynamic(WidgetId::from_raw(5), "初期値", read_count);
    assert_eq!(label.text(), "初期値");

    label.sync_from_state(&Counter { count: 1 });
    assert_eq!(label.text(), "count: 1");

    label.sync_from_state(&Counter { count: 2 });
    assert_eq!(label.text(), "count: 2");
}

/// `LabelWidget`（S11）のインタラクション状態遷移: `apply_input` による hover / pressed の
/// `visual_state()` 遷移を公開 API 経由で確認する（TASK-8.3-S12 / Issue #107）。
/// 優先順位（`Disabled > Pressed > Hovered > Idle`）は `InteractionState::visual_state`
/// （`widget.rs`）の契約に従う。`focus` は `visual_state()` に影響しない設計判断
/// （`widget.rs` の `focused_does_not_affect_visual_state` 参照）のため、フォーカス入力
/// 適用後も `Idle` のままであることを併せて固定化する。
#[test]
fn label_widget_apply_input_transitions_visual_state_via_public_api() {
    let mut label: LabelWidget<Counter> = LabelWidget::new_static(WidgetId::from_raw(6), "ラベル");

    use fandhe_core::VisualState;
    assert_eq!(label.visual_state(), VisualState::Idle);

    label.apply_input(InteractionInput::PointerEntered);
    assert_eq!(label.visual_state(), VisualState::Hovered);

    label.apply_input(InteractionInput::PointerPressed);
    assert_eq!(label.visual_state(), VisualState::Pressed);

    label.apply_input(InteractionInput::PointerReleased);
    assert_eq!(label.visual_state(), VisualState::Hovered);

    label.apply_input(InteractionInput::PointerExited);
    assert_eq!(label.visual_state(), VisualState::Idle);

    // フォーカス入力は視覚状態を変化させない（`widget.rs` の設計判断）。
    label.apply_input(InteractionInput::FocusGained);
    assert_eq!(label.visual_state(), VisualState::Idle);

    label.apply_input(InteractionInput::FocusLost);
    assert_eq!(label.visual_state(), VisualState::Idle);
}

/// `LabelWidget`（S11）の `set_disabled(true)` 後は、ポインタ・フォーカス入力が
/// すべて無視され `visual_state()` が `Disabled` に固定されることを公開 API 経由で
/// 確認する（TASK-8.3-S12 / Issue #107、`label_widget_disabled_ignores_pointer_and_focus_input`
/// のユニットテスト版と対の結合テスト）。
#[test]
fn label_widget_disabled_ignores_all_interaction_input_via_public_api() {
    let mut label: LabelWidget<Counter> = LabelWidget::new_static(WidgetId::from_raw(7), "ラベル");
    label.set_disabled(true);

    use fandhe_core::VisualState;
    label.apply_input(InteractionInput::PointerEntered);
    label.apply_input(InteractionInput::PointerPressed);
    label.apply_input(InteractionInput::FocusGained);

    assert_eq!(label.visual_state(), VisualState::Disabled);

    // disabled を解除すれば通常のインタラクション状態遷移へ戻る。
    label.set_disabled(false);
    assert_eq!(label.visual_state(), VisualState::Idle);
}

/// `Label` / `LabelWidget` の `accessibility_role` / `hit_test` 契約が、公開 API
/// （`Widget<S>` トレイト境界）を経由しても固定であることを確認する
/// （TASK-8.3-S12 / Issue #107）。ラベル・テキストは表示専用要素であり、常に
/// ポインタ透過（`hit_test` は常に `false`）・`AccessibilityRole::Label` を返す設計
/// 判断（`label.rs` 参照）。
#[test]
fn label_and_label_widget_accessibility_role_and_hit_test_are_fixed_via_trait_object() {
    let label: Box<dyn Widget<Counter>> = Box::new(Label::fixed(WidgetId::from_raw(8), "hello"));
    let label_widget: Box<dyn Widget<Counter>> =
        Box::new(LabelWidget::new_static(WidgetId::from_raw(9), "hello"));

    let size = Measurement {
        width: 100.0,
        height: 20.0,
    };

    for widget in [&label, &label_widget] {
        assert_eq!(widget.accessibility_role(), AccessibilityRole::Label);
        // 矩形内側の座標であってもラベルは常にポインタ透過。
        assert!(!widget.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!widget.hit_test((-10.0, -10.0), (0.0, 0.0), size));
    }
}

/// ハーネス経由の回帰: 複数ラベル（`Screen::label` × 2 + button）画面で、`tap` に
/// よるボタンイベント発火 → 状態バージョン更新 → 次フレームで `frames_rendered` が
/// 増える一方、スナップショットにウィジェット行が現れない（未配線契約の完全一致固定）
/// ことを検証する（TASK-8.3-S12 / Issue #107）。`Screen::label` を 2 件配置し、
/// 複数 `Node::Widget` が同時に存在する場合でも未配線契約が崩れないことを固定化する。
#[test]
fn multiple_labels_harness_regression_button_tap_updates_frame_count_without_widget_output() {
    fn read_count_label(state: &Counter) -> String {
        format!("count: {}", state.count)
    }
    fn read_doubled_label(state: &Counter) -> String {
        format!("doubled: {}", state.count * 2)
    }

    let screen: Screen<Counter> = Screen::new("multi-label")
        .label(WidgetId::from_raw(10), read_count_label)
        .label(WidgetId::from_raw(11), read_doubled_label)
        .button(WidgetId::from_raw(12), "increment", increment);

    let mut harness = WidgetHarness::new(screen, Counter::default());
    // `WidgetHarness` は自動レイアウトを持たないため、矩形はテスト側で明示的に
    // 登録する（`WidgetHarness` doc の `Examples` と同じ運用）。
    harness.set_regions(HitRegion::leaf(
        Rect::new(Point::new(0.0, 0.0), Size::new(20.0, 20.0)),
        "increment",
    ));

    let snapshot1 = harness.run_frame().to_vec();
    assert_eq!(harness.frames_rendered(), 1);
    // 未配線契約により、2 件のラベルはいずれもスナップショットへ出力されず、
    // ボタン行のみが現れる。
    assert_eq!(
        snapshot1,
        vec!["# multi-label".to_string(), "(increment)".to_string()]
    );

    // ボタンをタップして状態バージョンを更新する。
    assert!(harness.tap(Point::new(10.0, 10.0)));

    let snapshot2 = harness.run_frame().to_vec();
    // 状態バージョンが変化したため 2 回目の run_frame はスナップショットを
    // 再構築し、frames_rendered が増える（`frames_reused` は変化しない）。
    assert_eq!(harness.frames_rendered(), 2);
    assert_eq!(harness.frames_reused(), 0);
    // 状態が更新されても Node::Widget は未配線のプレースホルダのままのため、
    // スナップショット内容自体はラベル 2 件を含まず不変。
    assert_eq!(
        snapshot2,
        vec!["# multi-label".to_string(), "(increment)".to_string()]
    );
}
