//! プログレス表示ウィジェット `ProgressBar<S>` / `Spinner<S>`（TASK-8.3-S31 / REQ-8、
//! Issue #126）と、状態・イベント処理を担う `ProgressWidget<S>` / `ProgressMode`
//! （TASK-8.3-S32 / REQ-8、Issue #127）の結合テスト（TASK-8.3-S33 / REQ-8、Issue #128）。
//!
//! 親 issue #95（TASK-8.3）の一部。`Screen::progress_bar()` / `Screen::spinner()` が
//! 生成する `Node::Widget` が、既存の `WidgetHarness`（TASK-8.3-S5 / #100）を通じても
//! フレームループ・バージョン再利用の意味論（S5 契約）を壊さないこと、および
//! `render_headless`（`Node::Widget` プレースホルダ契約、`widget.rs` 参照）に対して
//! 未配線のまま安全側（無出力）であることを検証する。`tests/toggle_switch_widget.rs`
//! のパターンをプログレス表示向けに踏襲する。
//!
//! `progress.rs` 内の 46 件のユニットテストと重複する軸（`normalize_fraction` の
//! 詳細な境界値等）は避け、クレート外部から公開 API 経由で「`Screen` ビルダー →
//! `WidgetHarness` / `render_headless` の配線が壊れていないこと」「`ProgressBar` /
//! `Spinner` / `ProgressWidget` の公開契約（クランプ・no-op 条件・視覚状態）が
//! 外部から見ても成立すること」に絞って検証する。

use fandhe_core::{
    AccessibilityRole, LifecyclePhase, Measurement, Node, PaintCommand, ProgressBar, ProgressMode,
    ProgressWidget, Screen, Spinner, Widget, WidgetHarness, WidgetId, render_headless,
};

/// フォーム状態。`download_progress` フィールドを `ProgressBar` が読み取る想定。
#[derive(Default)]
struct FormState {
    download_progress: f32,
}

fn get_download_progress(state: &FormState) -> f32 {
    state.download_progress
}

fn progress_screen() -> Screen<FormState> {
    Screen::new("フォーム画面")
        .progress_bar(WidgetId::from_raw(1), "ダウンロード", get_download_progress)
        .spinner(WidgetId::from_raw(2))
}

// --- (1) WidgetHarness 駆動: S5 契約（バージョン不変ならスナップショット再利用） ---

/// `.progress_bar()` + `.spinner()` を含む `Screen` を `WidgetHarness` で駆動しても、
/// S5 契約（バージョン不変ならスナップショットを再利用する）が壊れないことを検証する
/// （REQ-8 / TASK-8.3-S33）。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_progress_screen() {
    let mut harness = WidgetHarness::new(progress_screen(), FormState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

// --- (2) 初回 run_frame のライフサイクル記録 ---

/// 初回 `run_frame` が `Declared` → `Mounted` → `Layout` → `Paint` の順でライフサイクル
/// フェーズを記録することを検証する（`toggle_switch_widget.rs` と同じ検証軸、
/// REQ-8 / TASK-8.3-S33）。
#[test]
fn progress_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(progress_screen(), FormState::default());
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

// --- (3) render_headless の未配線プレースホルダ契約 ---

/// `render_headless` スナップショットに `Node::Widget`（`ProgressBar` / `Spinner`）の
/// 行が現れないことを固定化する（未配線契約、`widget.rs` / `render.rs` 参照。
/// REQ-8 / TASK-8.3-S33）。
#[test]
fn render_headless_snapshot_does_not_contain_progress_widget_lines() {
    let screen = progress_screen();
    let state = FormState {
        download_progress: 0.75,
    };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、プログレスバー・スピナーいずれの行も出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.progress_bar()` / `.spinner()` 混在時も他バリアント（`.text()` 等）の描画・走査に
/// 影響しないことを確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の
/// 回帰防止。REQ-8 / TASK-8.3-S33）。
#[test]
fn progress_widgets_do_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面")
        .text("見出し")
        .progress_bar(WidgetId::from_raw(1), "ダウンロード", get_download_progress)
        .spinner(WidgetId::from_raw(2));
    let state = FormState::default();

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// `.progress_bar()` / `.spinner()` の 2 ノードがいずれも `Node::Widget` として
/// 追加されることを確認する（`toggle_switch_widget.rs` の
/// `toggle_switch_screen_builds_one_widget_node` と同じ観点）。
#[test]
fn progress_screen_builds_two_widget_nodes() {
    let screen = progress_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 2);
}

// --- (4) ProgressBar: sync_from_state / paint / hit_test / accessibility_role ---

/// `ProgressBar::sync_from_state` が状態から進捗値を読み取り、`[0.0, 1.0]` 外の値・
/// 非有限値（NaN/inf）を安全側へ正規化することを、クレート外部から公開 API 経由で
/// 確認する（`progress.rs` の `normalize_fraction` 契約、OWASP A04。
/// REQ-8 / TASK-8.3-S33）。
#[test]
fn progress_bar_sync_from_state_normalizes_out_of_range_and_non_finite_values() {
    let mut bar: ProgressBar<FormState> =
        ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_download_progress);

    bar.sync_from_state(&FormState {
        download_progress: -1.0,
    });
    assert_eq!(bar.fraction(), 0.0, "負値は 0.0 へクランプされるべき");

    bar.sync_from_state(&FormState {
        download_progress: 2.5,
    });
    assert_eq!(bar.fraction(), 1.0, "1 超の値は 1.0 へクランプされるべき");

    bar.sync_from_state(&FormState {
        download_progress: f32::NAN,
    });
    assert_eq!(bar.fraction(), 0.0, "NaN は 0.0 へフォールバックされるべき");

    bar.sync_from_state(&FormState {
        download_progress: f32::INFINITY,
    });
    assert_eq!(bar.fraction(), 0.0, "inf は 0.0 へフォールバックされるべき");
}

/// `ProgressBar::paint` のコマンド列が `[Rect（トラック）, （fraction > 0.0 のときのみ）
/// Rect（フィル）, Text(進捗率文字列), （label 非空時）Text(label)]` の契約
/// （`progress.rs` モジュール doc 参照）を、fraction == 0.0 / fraction > 0.0 の両方で
/// 満たすことを確認する（REQ-8 / TASK-8.3-S33）。
#[test]
fn progress_bar_paint_commands_reflect_fraction_and_label() {
    let mut bar: ProgressBar<FormState> =
        ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_download_progress);
    let available = Measurement {
        width: 200.0,
        height: 40.0,
    };

    bar.sync_from_state(&FormState {
        download_progress: 0.0,
    });
    let zero_commands = bar.paint((0.0, 0.0), bar.measure(available));
    assert_eq!(
        zero_commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Text("0%".to_string()),
            PaintCommand::Text("ダウンロード".to_string()),
        ],
        "fraction == 0.0 ではフィル Rect を含まないべき"
    );

    bar.sync_from_state(&FormState {
        download_progress: 0.42,
    });
    let filled_commands = bar.paint((0.0, 0.0), bar.measure(available));
    assert_eq!(
        filled_commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Rect,
            PaintCommand::Text("42%".to_string()),
            PaintCommand::Text("ダウンロード".to_string()),
        ],
        "fraction > 0.0 ではトラック・フィルの Rect 2 件を含むべき"
    );
}

/// `ProgressBar::hit_test` が常に `false` を返し、`accessibility_role` が
/// `ProgressIndicator` であることを確認する（表示専用ウィジェットの契約、
/// REQ-8 / TASK-8.3-S33）。
#[test]
fn progress_bar_is_non_interactive_with_progress_indicator_role() {
    let bar: ProgressBar<FormState> =
        ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_download_progress);
    let available = Measurement {
        width: 200.0,
        height: 40.0,
    };

    assert!(!bar.hit_test((10.0, 10.0), (0.0, 0.0), available));
    assert_eq!(
        bar.accessibility_role(),
        AccessibilityRole::ProgressIndicator
    );
}

// --- (5) Spinner: sync_from_state no-op / paint / measure ---

/// `Spinner::sync_from_state` が状態バインディングを持たない no-op であること
/// （状態が変化しても `paint` の出力が変わらないこと）を確認する
/// （REQ-8 / TASK-8.3-S33）。
#[test]
fn spinner_sync_from_state_is_no_op() {
    let mut spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));
    let available = Measurement {
        width: 40.0,
        height: 40.0,
    };

    spinner.sync_from_state(&FormState {
        download_progress: 0.0,
    });
    let before = spinner.paint((0.0, 0.0), spinner.measure(available));

    spinner.sync_from_state(&FormState {
        download_progress: 1.0,
    });
    let after = spinner.paint((0.0, 0.0), spinner.measure(available));

    assert_eq!(
        before, after,
        "Spinner は状態バインディングを持たないため sync_from_state は no-op であるべき"
    );
    assert_eq!(
        before,
        vec![
            PaintCommand::Rect,
            PaintCommand::Text("\u{27F3}".to_string())
        ]
    );
}

/// `Spinner::measure` が `available` でクランプされることを確認する
/// （REQ-8 / TASK-8.3-S33）。
#[test]
fn spinner_measure_clamps_to_available_space() {
    let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));

    let ample = spinner.measure(Measurement {
        width: 100.0,
        height: 100.0,
    });
    assert!(ample.width > 0.0 && ample.height > 0.0);

    let constrained = spinner.measure(Measurement {
        width: 4.0,
        height: 4.0,
    });
    assert_eq!(constrained.width, 4.0);
    assert_eq!(constrained.height, 4.0);
}

// --- (6) ProgressWidget: new_determinate / new_indeterminate / advance / set_disabled ---

/// `ProgressWidget::new_determinate` / `new_indeterminate` がそれぞれ意図した
/// `ProgressMode` で構築されることを確認する（REQ-8 / TASK-8.3-S33）。
#[test]
fn progress_widget_new_constructors_set_expected_mode() {
    let determinate: ProgressWidget<FormState> =
        ProgressWidget::new_determinate(WidgetId::from_raw(1), get_download_progress);
    assert_eq!(determinate.mode(), ProgressMode::Determinate);

    let indeterminate: ProgressWidget<FormState> =
        ProgressWidget::new_indeterminate(WidgetId::from_raw(2));
    assert_eq!(indeterminate.mode(), ProgressMode::Indeterminate);
}

/// `ProgressWidget::advance` が indeterminate モードで位相を `[0.0, 1.0)` の範囲で
/// 循環前進させること、負値・非有限 `elapsed_secs` では no-op となること（OWASP A04）、
/// determinate モードでは常に no-op となることを確認する（REQ-8 / TASK-8.3-S33）。
#[test]
fn progress_widget_advance_cycles_phase_and_ignores_invalid_elapsed() {
    let mut spinner: ProgressWidget<FormState> =
        ProgressWidget::new_indeterminate(WidgetId::from_raw(2));

    spinner.advance(0.25);
    assert_eq!(spinner.phase(), 0.25);

    // 0.9 を加算すると 1.0 を跨ぐため [0.0, 1.0) の範囲で巻き戻るはず。
    spinner.advance(0.9);
    assert!((spinner.phase() - 0.15).abs() < 1e-5);

    spinner.advance(-1.0);
    assert!(
        (spinner.phase() - 0.15).abs() < 1e-5,
        "負値の elapsed_secs は no-op であるべき"
    );

    spinner.advance(f32::NAN);
    assert!(
        (spinner.phase() - 0.15).abs() < 1e-5,
        "非有限値の elapsed_secs は no-op であるべき"
    );

    let mut bar: ProgressWidget<FormState> =
        ProgressWidget::new_determinate(WidgetId::from_raw(1), get_download_progress);
    bar.advance(0.5);
    assert_eq!(
        bar.phase(),
        0.0,
        "determinate モードでは advance は no-op であるべき"
    );
}

/// `ProgressWidget::set_disabled` が `visual_state` へ反映されることを確認する
/// （`InteractionState::set_disabled` への委譲、REQ-8 / TASK-8.3-S33）。
#[test]
fn progress_widget_set_disabled_updates_visual_state() {
    use fandhe_core::VisualState;

    let mut widget: ProgressWidget<FormState> =
        ProgressWidget::new_determinate(WidgetId::from_raw(1), get_download_progress);
    assert_eq!(widget.visual_state(), VisualState::Idle);

    widget.set_disabled(true);
    assert_eq!(widget.visual_state(), VisualState::Disabled);

    widget.set_disabled(false);
    assert_eq!(widget.visual_state(), VisualState::Idle);
}
