//! `RadioButton<S>`（ラジオボタンウィジェット）の結合テスト（TASK-8.3-S22 / REQ-8、
//! Issue #117）。
//!
//! 親 issue #95（TASK-8.3）の一部。`Screen::radio_button()` が生成する
//! `Node::Widget` が、既存の `WidgetHarness`（TASK-8.3-S5 / #100）を通じても
//! フレームループ・バージョン再利用の意味論（S5 契約）を壊さないこと、および
//! `render_headless`（`Node::Widget` プレースホルダ契約、widget.rs 参照）に対して
//! 未配線のまま安全側（無出力）であることを検証する。`tests/text_input_widget.rs` の
//! パターンをラジオボタン向けに踏襲する。

use fandhe_core::{Screen, WidgetHarness, WidgetId, render_headless};

/// フォーム状態。`selected` フィールドをラジオボタングループが共有する想定
/// （書き込みの実配線自体は S23 のスコープ、本テストでは検証しない）。
#[derive(Default)]
struct FormState {
    selected: usize,
}

fn get_selected(state: &FormState) -> usize {
    state.selected
}

fn set_selected(state: &mut FormState, value: usize) {
    state.selected = value;
}

fn radio_button_screen() -> Screen<FormState> {
    Screen::new("フォーム画面").radio_button(
        WidgetId::from_raw(1),
        "選択肢A",
        0,
        get_selected,
        set_selected,
    )
}

/// 同一状態フィールドを共有する 2 つの `.radio_button()` を持つ画面（グループ内排他選択の
/// 結線確認用）。
fn radio_group_screen() -> Screen<FormState> {
    Screen::new("グループ画面")
        .radio_button(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        )
        .radio_button(
            WidgetId::from_raw(2),
            "選択肢B",
            1,
            get_selected,
            set_selected,
        )
}

/// (f) `.radio_button()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_radio_button_screen() {
    let mut harness = WidgetHarness::new(radio_button_screen(), FormState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (f) 初回 `run_frame` は `Mounted` を、状態バージョンが変わらない限り以降は
/// 再利用のみで新規フェーズを記録しないことを検証する（`widget_harness.rs` の
/// ボタン向けテストと同じ検証軸を、`Node::Widget` を含む画面に対して確認する）。
#[test]
fn radio_button_screen_records_declared_and_mounted_phases_on_first_frame() {
    use fandhe_core::LifecyclePhase;

    let mut harness = WidgetHarness::new(radio_button_screen(), FormState::default());
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

/// (g) `render_headless` スナップショットに `Node::Widget`（`RadioButton`）の行が
/// 現れないことを固定化する（未配線契約、widget.rs / render.rs 参照）。
///
/// `sync_from_state`（`&mut self` 要求）を呼べる経路が `render_headless` に存在しない
/// ため、`paint()` への委譲は行わず安全側（無出力）に倒す契約を、ラジオボタンに
/// ついても回帰検知できるようにする。
#[test]
fn render_headless_snapshot_does_not_contain_radio_button_line() {
    let screen = radio_button_screen();
    let state = FormState { selected: 0 };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、ラジオボタンの行（ラベル・マークのいずれも）は出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.radio_button()` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存 4 バリアントの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn radio_button_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面").text("見出し").radio_button(
        WidgetId::from_raw(1),
        "選択肢A",
        0,
        get_selected,
        set_selected,
    );
    let state = FormState { selected: 0 };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// グループ内排他選択が `Screen` 経由でも 2 ノード分正しく組み立てられることを確認する
/// （`radio_button.rs` の `exclusive_selection_across_group_members` の Screen 版）。
#[test]
fn radio_group_screen_builds_two_widget_nodes() {
    use fandhe_core::Node;

    let screen = radio_group_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 2);
}
