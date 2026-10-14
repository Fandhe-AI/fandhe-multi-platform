//! `ToggleSwitch<S>`（トグルスイッチウィジェット）の結合テスト（TASK-8.3-S27 / REQ-8、
//! Issue #122）。
//!
//! 親 issue #95（TASK-8.3）の一部。`Screen::toggle_switch()` が生成する `Node::Widget`
//! が、既存の `WidgetHarness`（TASK-8.3-S5 / #100）を通じてもフレームループ・
//! バージョン再利用の意味論（S5 契約）を壊さないこと、および `render_headless`
//! （`Node::Widget` プレースホルダ契約、`widget.rs` 参照）に対して未配線のまま安全側
//! （無出力）であることを検証する。`tests/radio_button_widget.rs` のパターンを
//! トグルスイッチ向けに踏襲する。
//!
//! `ToggleSwitchInput` / `apply_input` 等のイベント配線は本ファイルの対象外
//! （Issue #121 のスコープ）のため、`ToggleSwitch::bound` / `Widget` トレイト直接
//! 呼び出しの範囲で検証する。

use fandhe_core::{
    LifecyclePhase, Measurement, Node, PaintCommand, Screen, ToggleSwitch, Widget, WidgetHarness,
    WidgetId, render_headless,
};

/// フォーム状態。`notifications_on` フィールドをトグルスイッチが読み書きする想定
/// （書き込みの実配線自体は S26 のスコープ、本テストでは検証しない）。
#[derive(Default)]
struct FormState {
    notifications_on: bool,
}

fn get_notifications_on(state: &FormState) -> bool {
    state.notifications_on
}

fn set_notifications_on(state: &mut FormState, value: bool) {
    state.notifications_on = value;
}

fn toggle_switch_screen() -> Screen<FormState> {
    Screen::new("フォーム画面").toggle_switch(
        WidgetId::from_raw(1),
        "通知",
        get_notifications_on,
        set_notifications_on,
    )
}

/// (1) `.toggle_switch()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_toggle_switch_screen() {
    let mut harness = WidgetHarness::new(toggle_switch_screen(), FormState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (2) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`radio_button_widget.rs` と同じ
/// 検証軸を、`ToggleSwitch` を含む画面に対して確認する）。
#[test]
fn toggle_switch_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(toggle_switch_screen(), FormState::default());
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

/// (3) `render_headless` スナップショットに `Node::Widget`（`ToggleSwitch`）の行が
/// 現れないことを固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。
///
/// `sync_from_state`（`&mut self` 要求）を呼べる経路が `render_headless` に存在しない
/// ため、`paint()` への委譲は行わず安全側（無出力）に倒す契約を、トグルスイッチについても
/// 回帰検知できるようにする。
#[test]
fn render_headless_snapshot_does_not_contain_toggle_switch_line() {
    let screen = toggle_switch_screen();
    let state = FormState {
        notifications_on: true,
    };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、トグルスイッチの行（ラベル・ON マークのいずれも）は出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.toggle_switch()` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn toggle_switch_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面").text("見出し").toggle_switch(
        WidgetId::from_raw(1),
        "通知",
        get_notifications_on,
        set_notifications_on,
    );
    let state = FormState {
        notifications_on: false,
    };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// (4) `sync_from_state` 経由の on/off キャッシュ同期と `paint` コマンド列の差分
/// （`ON` マーク有無）を、`Widget` トレイトを直接呼び出して検証する。`Screen` から
/// 取り出した `Node::Widget` に対し `&mut dyn Widget<FormState>` を得るため、
/// `Node::Widget` の中身をテスト内で直接構築する（`render_headless` は `&mut` を
/// 届けない契約のため、この経路は本テスト固有の直接駆動である）。
#[test]
fn sync_from_state_toggles_on_mark_in_paint_commands() {
    let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
        WidgetId::from_raw(1),
        "通知",
        get_notifications_on,
        set_notifications_on,
    );
    let available = Measurement {
        width: 200.0,
        height: 40.0,
    };

    let off_state = FormState {
        notifications_on: false,
    };
    toggle.sync_from_state(&off_state);
    let off_commands = toggle.paint((0.0, 0.0), toggle.measure(available));
    assert!(
        !off_commands
            .iter()
            .any(|command| matches!(command, PaintCommand::Text(text) if text == "ON")),
        "off 状態では ON マークを含まないべき"
    );

    let on_state = FormState {
        notifications_on: true,
    };
    toggle.sync_from_state(&on_state);
    let on_commands = toggle.paint((0.0, 0.0), toggle.measure(available));
    assert!(
        on_commands
            .iter()
            .any(|command| matches!(command, PaintCommand::Text(text) if text == "ON")),
        "on 状態では ON マークを含むべき"
    );
}

/// `Node::Widget` 経由で組み立てたトグルスイッチが 1 ノード分正しく追加されることを
/// 確認する（`radio_button_widget.rs` の `radio_group_screen_builds_two_widget_nodes` と
/// 同じ観点、トグルスイッチはグループ排他選択を持たないため単体のみ検証する）。
#[test]
fn toggle_switch_screen_builds_one_widget_node() {
    let screen = toggle_switch_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}
