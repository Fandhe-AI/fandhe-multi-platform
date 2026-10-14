//! `Icon<S>`（アイコンウィジェット）の結合テスト（TASK-8.3-S39 / REQ-8、Issue #134）。
//!
//! 親 issue #95（TASK-8.3）の一部。`Screen::icon()` / `Screen::icon_bound()` が
//! 生成する `Node::Widget` が、既存の `WidgetHarness`（TASK-8.3-S5 / #100）を通じても
//! フレームループ・バージョン再利用の意味論（S5 契約）を壊さないこと、および
//! `render_headless`（`Node::Widget` プレースホルダ契約、`widget.rs` 参照）に対して
//! 未配線のまま安全側（無出力）であることを検証する。`tests/image_widget.rs` のパターンを
//! アイコン向けに踏襲する。
//!
//! `Icon<S>` 本体は `Widget<S>` のみに依存する自己完結の最小実装（`src/ui/icon.rs`、
//! Issue #132 が定める API 面に合わせたもの）。本テストは `Icon::fixed` / `Icon::bound` /
//! `Widget` トレイト直接呼び出しの範囲でのみ検証し、タップ可能アイコン等の状態遷移・
//! イベント処理は本ファイルの対象外（Issue #133 のスコープ）。

use fandhe_core::{
    Icon, LifecyclePhase, Measurement, Node, PaintCommand, Screen, Widget, WidgetHarness, WidgetId,
    render_headless,
};

/// 設定画面の状態。`active_icon` フィールドをアイコンウィジェットが読み取る想定。
#[derive(Default)]
struct SettingsState {
    active_icon: String,
}

fn get_active_icon(state: &SettingsState) -> String {
    state.active_icon.clone()
}

fn icon_screen() -> Screen<SettingsState> {
    Screen::new("設定画面").icon_bound(WidgetId::from_raw(1), get_active_icon, "現在のアイコン")
}

/// (1) `.icon_bound()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_icon_screen() {
    let mut harness = WidgetHarness::new(icon_screen(), SettingsState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (2) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`image_widget.rs` と同じ
/// 検証軸を、`Icon` を含む画面に対して確認する）。
#[test]
fn icon_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(icon_screen(), SettingsState::default());
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

/// (3) `render_headless` スナップショットに `Node::Widget`（`Icon`）の行が現れないことを
/// 固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。
///
/// `sync_from_state`（`&mut self` 要求）を呼べる経路が `render_headless` に存在しないため、
/// `paint()` への委譲は行わず安全側（無出力）に倒す契約を、アイコンについても回帰検知
/// できるようにする。
#[test]
fn render_headless_snapshot_does_not_contain_icon_line() {
    let screen = icon_screen();
    let state = SettingsState {
        active_icon: "settings".to_string(),
    };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、アイコンの行（ラベル・プレースホルダのいずれも）は出力されない。
    assert_eq!(snapshot, vec!["# 設定画面".to_string()]);
}

/// `.icon()` / `.icon_bound()` 混在時も他バリアント（`.text()` 等）の描画・走査に
/// 影響しないことを確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の
/// 回帰防止）。
#[test]
fn icon_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<SettingsState> =
        Screen::new("混在画面")
            .text("見出し")
            .icon(WidgetId::from_raw(1), "settings", "設定");
    let state = SettingsState {
        active_icon: String::new(),
    };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// (4) `sync_from_state` 経由のアイコン名キャッシュ更新と `paint` コマンド列の内容
/// （ラベルのみが `Text` に現れ、アイコン名は現れないこと）を、`Widget` トレイトを
/// 直接呼び出して検証する。`Screen` から取り出した `Node::Widget` に対し
/// `&mut dyn Widget<SettingsState>` を得るため、`Node::Widget` の中身をテスト内で直接構築する
/// （`render_headless` は `&mut` を届けない契約のため、この経路は本テスト固有の直接駆動で
/// ある）。
#[test]
fn sync_from_state_updates_cached_name_and_paint_reflects_label_only() {
    let mut icon: Icon<SettingsState> =
        Icon::bound(WidgetId::from_raw(1), get_active_icon, "現在のアイコン");
    let available = Measurement {
        width: 200.0,
        height: 200.0,
    };

    let state = SettingsState {
        active_icon: "secret-icon-name".to_string(),
    };
    icon.sync_from_state(&state);
    assert_eq!(icon.name_text(), "secret-icon-name");

    let commands = icon.paint((0.0, 0.0), icon.measure(available));
    assert_eq!(
        commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Text("現在のアイコン".to_string()),
        ]
    );
    for command in &commands {
        if let PaintCommand::Text(text) = command {
            assert!(!text.contains("secret-icon-name"));
        }
    }
}

/// `Node::Widget` 経由で組み立てたアイコンが 1 ノード分正しく追加されることを確認する
/// （`image_widget.rs` の `image_screen_builds_one_widget_node` と同じ観点）。
#[test]
fn icon_screen_builds_one_widget_node() {
    let screen = icon_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}
