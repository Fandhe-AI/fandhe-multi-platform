//! `Checkbox<S>` / `CheckboxWidget<S>`（チェックボックスウィジェット）の結合テスト
//! （TASK-8.3-S21 / REQ-8、Issue #116）。
//!
//! 親 issue #95（TASK-8.3）の一部。`Screen::checkbox()` が生成する `Node::Widget` が、
//! 既存の `WidgetHarness`（TASK-8.3-S5 / #100）を通じてもフレームループ・バージョン
//! 再利用の意味論（S5 契約）を壊さないこと、および `render_headless`（`Node::Widget`
//! プレースホルダ契約、`widget.rs` 参照）に対して未配線のまま安全側（無出力）であることを
//! 検証する。`tests/toggle_switch_widget.rs` / `tests/radio_button_widget.rs` のパターンを
//! チェックボックス向けに踏襲する。
//!
//! クレート外部の公開 re-export（`fandhe_core::{CheckboxWidget, CheckboxInput}`）経由での
//! イベント疎通も併せて最小検証する（`checkbox.rs` の inline tests と重複しすぎない
//! 範囲に留める）。

use fandhe_core::{
    Checkbox, CheckboxInput, CheckboxWidget, InteractionInput, LifecyclePhase, Measurement, Node,
    PaintCommand, Screen, Widget, WidgetHarness, WidgetId, render_headless,
};

/// フォーム状態。`agreed` フィールドをチェックボックスが読み書きする想定
/// （書き込みの実配線自体は `WidgetId` ベースディスパッチ確立後のスコープ、本テストでは
/// `Widget<S>` トレイト直接呼び出しの範囲で検証する）。
#[derive(Default)]
struct FormState {
    agreed: bool,
}

fn get_agreed(state: &FormState) -> bool {
    state.agreed
}

fn set_agreed(state: &mut FormState, value: bool) {
    state.agreed = value;
}

fn checkbox_screen() -> Screen<FormState> {
    Screen::new("フォーム画面").checkbox(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed)
}

/// (1) `.checkbox()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_checkbox_screen() {
    let mut harness = WidgetHarness::new(checkbox_screen(), FormState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (2) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`toggle_switch_widget.rs` と同じ
/// 検証軸を、`Checkbox` を含む画面に対して確認する）。
#[test]
fn checkbox_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(checkbox_screen(), FormState::default());
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

/// (3) `render_headless` スナップショットに `Node::Widget`（`Checkbox`）の行が
/// 現れないことを固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。
///
/// `sync_from_state`（`&mut self` 要求）を呼べる経路が `render_headless` に存在しない
/// ため、`paint()` への委譲は行わず安全側（無出力）に倒す契約を、チェックボックスに
/// ついても回帰検知できるようにする。
#[test]
fn render_headless_snapshot_does_not_contain_checkbox_line() {
    let screen = checkbox_screen();
    let state = FormState { agreed: true };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、チェックボックスの行（ラベル・チェックマークのいずれも）は
    // 出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// (4) `.checkbox()` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn checkbox_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面").text("見出し").checkbox(
        WidgetId::from_raw(1),
        "同意する",
        get_agreed,
        set_agreed,
    );
    let state = FormState { agreed: false };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// (5) `sync_from_state` 経由のチェック状態キャッシュ同期と `paint` コマンド列の差分
/// （チェックマーク `✓` 有無）を、`Widget` トレイトを直接呼び出して検証する（checked /
/// unchecked 双方）。`Screen` から取り出した `Node::Widget` に対し
/// `&mut dyn Widget<FormState>` を得るため、`Node::Widget` の中身をテスト内で直接構築する
/// （`render_headless` は `&mut` を届けない契約のため、この経路は本テスト固有の
/// 直接駆動である）。
#[test]
fn sync_from_state_toggles_check_mark_in_paint_commands() {
    let mut checkbox: Checkbox<FormState> =
        Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);
    let available = Measurement {
        width: 200.0,
        height: 40.0,
    };

    let unchecked_state = FormState { agreed: false };
    checkbox.sync_from_state(&unchecked_state);
    let unchecked_commands = checkbox.paint((0.0, 0.0), checkbox.measure(available));
    assert!(
        !unchecked_commands
            .iter()
            .any(|command| matches!(command, PaintCommand::Text(text) if text == "\u{2713}")),
        "unchecked 状態ではチェックマークを含まないべき"
    );

    let checked_state = FormState { agreed: true };
    checkbox.sync_from_state(&checked_state);
    let checked_commands = checkbox.paint((0.0, 0.0), checkbox.measure(available));
    assert!(
        checked_commands
            .iter()
            .any(|command| matches!(command, PaintCommand::Text(text) if text == "\u{2713}")),
        "checked 状態ではチェックマークを含むべき"
    );
}

/// (6) `.checkbox()` で組み立てた `Screen` が `Node::Widget` をちょうど 1 件生成し、
/// その `accessibility_role()` が `AccessibilityRole::CheckBox` であることを確認する
/// （`radio_button_widget.rs` の `radio_group_screen_builds_two_widget_nodes` と同じ観点、
/// チェックボックスはグループ排他選択を持たないため単体のみ検証する）。
#[test]
fn checkbox_screen_builds_one_widget_node_with_checkbox_role() {
    use fandhe_core::AccessibilityRole;

    let screen = checkbox_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(widget) = node {
            count += 1;
            assert_eq!(widget.accessibility_role(), AccessibilityRole::CheckBox);
        }
    });

    assert_eq!(count, 1);
}

/// (7) S20（#115）が確定済みのため、`CheckboxWidget<S>` のイベント経路も外部公開 API
/// として最小検証する（inline tests との重複を避け、公開 re-export の疎通確認に留める）。
///
/// ポインタ press→release でのトグル確定を検証する。`apply_input` はクリック確定の
/// 判定を `PointerReleased` 適用前の `pressed && hovered` で行う契約（`checkbox.rs`
/// 参照）のため、`PointerEntered` → `PointerPressed` → `PointerReleased` の順で入力する。
#[test]
fn checkbox_widget_pointer_press_release_confirms_toggle_via_public_api() {
    let mut widget: CheckboxWidget<FormState> =
        CheckboxWidget::new_static(WidgetId::from_raw(1), "同意する", false);

    assert_eq!(widget.apply_input(InteractionInput::PointerEntered), None);
    assert_eq!(widget.apply_input(InteractionInput::PointerPressed), None);
    let event = widget.apply_input(InteractionInput::PointerReleased);
    assert_eq!(
        event,
        Some(fandhe_core::Event::ButtonClicked {
            label: "同意する".to_string()
        })
    );
}

/// (8) disabled 中は `CheckboxWidget<S>` へのキーボード入力（`CheckboxInput::Toggle`）が
/// すべて無視されることを検証する（`checkbox.rs` の disabled 意味論を公開 API 越しに
/// 疎通確認する）。
#[test]
fn checkbox_widget_ignores_keyboard_input_while_disabled_via_public_api() {
    let mut widget: CheckboxWidget<FormState> =
        CheckboxWidget::new_static(WidgetId::from_raw(1), "同意する", false);
    widget.apply_input(InteractionInput::FocusGained);
    widget.set_disabled(true);

    let event = widget.handle_input(CheckboxInput::Toggle);

    assert_eq!(event, None, "disabled 中はキーボード入力を無視するべき");
}
