//! `Slider<S>`（スライダーウィジェット）の結合テスト（TASK-8.3-S30 / REQ-8、Issue #125）。
//!
//! 親 issue #95（TASK-8.3）の一部。`Screen::slider()` が生成する `Node::Widget` が、
//! 既存の `WidgetHarness`（TASK-8.3-S5 / #100）を通じてもフレームループ・バージョン
//! 再利用の意味論（S5 契約）を壊さないこと、および `render_headless`（`Node::Widget`
//! プレースホルダ契約、`widget.rs` 参照）に対して未配線のまま安全側（無出力）であることを
//! 検証する。`tests/toggle_switch_widget.rs` のパターンをスライダー向けに踏襲する。
//!
//! `SliderInput` / `apply_input` 等のイベント配線は本ファイルの対象外（Issue #124 の
//! スコープ）のため、`Slider::bound` / `with_step` / `Widget` トレイト直接呼び出しの
//! 範囲で検証する。

use fandhe_core::{
    LifecyclePhase, Measurement, Node, PaintCommand, Screen, Slider, Widget, WidgetHarness,
    WidgetId, render_headless,
};

/// フォーム状態。`volume` フィールドをスライダーが読み書きする想定
/// （書き込みの実配線自体は S29 のスコープ、本テストでは検証しない）。
#[derive(Default)]
struct FormState {
    volume: f32,
}

fn get_volume(state: &FormState) -> f32 {
    state.volume
}

fn set_volume(state: &mut FormState, value: f32) {
    state.volume = value;
}

fn slider_screen() -> Screen<FormState> {
    Screen::new("フォーム画面").slider(
        WidgetId::from_raw(1),
        "音量",
        0.0,
        100.0,
        get_volume,
        set_volume,
    )
}

/// (1) `.slider()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_slider_screen() {
    let mut harness = WidgetHarness::new(slider_screen(), FormState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (2) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`toggle_switch_widget.rs` と同じ
/// 検証軸を、`Slider` を含む画面に対して確認する）。
#[test]
fn slider_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(slider_screen(), FormState::default());
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

/// (3) `render_headless` スナップショットに `Node::Widget`（`Slider`）の行が
/// 現れないことを固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。
///
/// `sync_from_state`（`&mut self` 要求）を呼べる経路が `render_headless` に存在しない
/// ため、`paint()` への委譲は行わず安全側（無出力）に倒す契約を、スライダーについても
/// 回帰検知できるようにする。
#[test]
fn render_headless_snapshot_does_not_contain_slider_line() {
    let screen = slider_screen();
    let state = FormState { volume: 42.0 };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、スライダーの行（値・ラベルのいずれも）は出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.slider()` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn slider_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面").text("見出し").slider(
        WidgetId::from_raw(1),
        "音量",
        0.0,
        100.0,
        get_volume,
        set_volume,
    );
    let state = FormState { volume: 0.0 };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// (4) `sync_from_state` 経由の値同期と `paint` コマンド列（値テキスト・フィル済み
/// トラック矩形）の差分を、`Widget` トレイトを直接呼び出して検証する。`Screen` から
/// 取り出した `Node::Widget` に対し `&mut dyn Widget<FormState>` を得るため、
/// `Node::Widget` の中身をテスト内で直接構築する（`render_headless` は `&mut` を
/// 届けない契約のため、この経路は本テスト固有の直接駆動である）。
#[test]
fn sync_from_state_updates_value_and_fill_in_paint_commands() {
    let mut slider: Slider<FormState> = Slider::bound(
        WidgetId::from_raw(1),
        "音量",
        0.0,
        100.0,
        get_volume,
        set_volume,
    );
    let available = Measurement {
        width: 200.0,
        height: 20.0,
    };

    let low_state = FormState { volume: 10.0 };
    slider.sync_from_state(&low_state);
    assert_eq!(slider.value(), 10.0);
    assert_eq!(slider.ratio(), 0.1);
    let low_commands = slider.paint((0.0, 0.0), slider.measure(available));
    assert!(
        low_commands
            .iter()
            .any(|command| matches!(command, PaintCommand::Text(text) if text == "10.0/100.0")),
        "低い値ではその値を反映したテキストを含むべき"
    );

    let high_state = FormState { volume: 80.0 };
    slider.sync_from_state(&high_state);
    assert_eq!(slider.value(), 80.0);
    assert_eq!(slider.ratio(), 0.8);
    let high_commands = slider.paint((0.0, 0.0), slider.measure(available));
    assert!(
        high_commands
            .iter()
            .any(|command| matches!(command, PaintCommand::Text(text) if text == "80.0/100.0")),
        "高い値ではその値を反映したテキストを含むべき"
    );
    assert_ne!(
        low_commands, high_commands,
        "値が変われば paint コマンド列も変化するべき（フィル済みトラック・値テキストの差分）"
    );
}

/// `Node::Widget` 経由で組み立てたスライダーが 1 ノード分正しく追加されることを
/// 確認する（`toggle_switch_widget.rs` の `toggle_switch_screen_builds_one_widget_node`
/// と同じ観点）。
#[test]
fn slider_screen_builds_one_widget_node() {
    let screen = slider_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}

/// (7) 境界値の検証: `min > max` を入れ替えて正規化すること（S28 の数値健全性契約、
/// `slider.rs` の `normalize_range` doc 参照）を公開 API 経由（`Slider::bound`）で
/// 固定化する。`slider.rs` 内のユニットテストと同じ観点だが、クレート外部（`tests/`）の
/// 公開 API 面からも回帰検知できるようにする。
#[test]
fn bound_swaps_inverted_min_max_via_public_api() {
    let slider: Slider<FormState> = Slider::bound(
        WidgetId::from_raw(1),
        "音量",
        100.0,
        0.0,
        get_volume,
        set_volume,
    );

    assert_eq!(slider.min(), 0.0);
    assert_eq!(slider.max(), 100.0);
}

/// (7) 境界値の検証: `min` / `max` が非有限値（NaN/inf）の場合は既定範囲
/// （`[0.0, 1.0]`）へフォールバックすることを公開 API 経由で固定化する。
#[test]
fn bound_falls_back_to_default_range_for_non_finite_bounds_via_public_api() {
    let slider: Slider<FormState> = Slider::bound(
        WidgetId::from_raw(1),
        "音量",
        f32::NAN,
        f32::INFINITY,
        get_volume,
        set_volume,
    );

    assert_eq!(slider.min(), 0.0);
    assert_eq!(slider.max(), 1.0);
}

/// (7) 境界値の検証: `with_step(0.0)` は連続値（`None`）のまま扱うことを公開 API 経由で
/// 固定化する（`step <= 0.0` は無視する契約、`slider.rs` の `normalize_step` doc 参照）。
#[test]
fn with_step_zero_keeps_continuous_value_via_public_api() {
    let slider: Slider<FormState> = Slider::bound(
        WidgetId::from_raw(1),
        "音量",
        0.0,
        100.0,
        get_volume,
        set_volume,
    )
    .with_step(0.0);

    assert_eq!(slider.step(), None);
}
