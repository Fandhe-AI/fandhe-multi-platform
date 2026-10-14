//! `ScrollView<S>`（スクロールビューウィジェット）の結合テスト（TASK-8.3-S42 / REQ-8、
//! Issue #137）。
//!
//! 親 issue #95（TASK-8.3）の一部。`Screen::scroll_view()` が生成する `Node::Widget`
//! が、既存の `WidgetHarness`（TASK-8.3-S5 / #100）を通じてもフレームループ・
//! バージョン再利用の意味論（S5 契約）を壊さないこと、および `render_headless`
//! （`Node::Widget` プレースホルダ契約、`widget.rs` 参照）に対して未配線のまま安全側
//! （無出力）であることを検証する。`tests/icon_widget.rs` のパターンをスクロール
//! ビュー向けに踏襲する。
//!
//! `ScrollView<S>` は単一の子 `Widget<S>` をスクロール可能なビューポート内に配置する
//! コンテナ型 API で、`ScrollAxis` でスクロール可能軸を指定する（Issue #135）。
//! 本テストはこのコンテナ型 API のみを対象とし、状態束縛のスクロールオフセットは
//! 本ファイルの対象外（Issue #136 のスコープ）。

use fandhe_core::{
    Label, LifecyclePhase, Measurement, Node, PaintCommand, Screen, ScrollAxis, ScrollView, Widget,
    WidgetHarness, WidgetId, render_headless,
};

/// フィード画面の状態。スクロールビュー自体は本ファイルの対象範囲では状態を読み取らない
/// （子コンテンツの `Label::fixed` も同様に固定文字列のみを扱う）。
#[derive(Default)]
struct FeedState;

fn feed_screen() -> Screen<FeedState> {
    Screen::new("フィード画面").scroll_view(
        WidgetId::from_raw(1),
        Label::fixed(WidgetId::from_raw(2), "フィード一覧"),
    )
}

/// (1) `.scroll_view()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_scroll_view_screen() {
    let mut harness = WidgetHarness::new(feed_screen(), FeedState);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (2) 初回 `run_frame` は `Declared` → `Mounted` → `Layout` → `Paint` の順で
/// ライフサイクルフェーズを記録することを検証する（`icon_widget.rs` と同じ検証軸を、
/// `ScrollView` を含む画面に対して確認する）。
#[test]
fn scroll_view_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(feed_screen(), FeedState);
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

/// (3) `render_headless` スナップショットに `Node::Widget`（`ScrollView`）の行が
/// 現れないことを固定化する（未配線契約、`widget.rs` / `render.rs` 参照）。
#[test]
fn render_headless_snapshot_does_not_contain_scroll_view_line() {
    let screen = feed_screen();
    let state = FeedState;

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、スクロールビューの行（ラベル・プレースホルダのいずれも）は
    // 出力されない。
    assert_eq!(snapshot, vec!["# フィード画面".to_string()]);
}

/// `.scroll_view()` を使った画面でも他バリアント（`.text()` 等）の描画・走査に
/// 影響しないことを確認する（既存バリアントとの後方互換性、`Node::Widget` 追加時の
/// 回帰防止）。
#[test]
fn scroll_view_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FeedState> = Screen::new("混在画面").text("見出し").scroll_view(
        WidgetId::from_raw(1),
        Label::fixed(WidgetId::from_raw(2), "フィード"),
    );
    let state = FeedState;

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

/// `set_offset_clamps_across_boundary_inputs_via_effective_offset` 専用の固定計測
/// コンテンツ（Bugbot 指摘対応: PR #383 レビュー、`crates/fandhe-core/tests/scroll_view_widget.rs`
/// L106-L134）。`Label` の測定は常に固定行高（`APPROX_LINE_HEIGHT`）を返すため
/// ビューポートを超えず、上限クランプ（`effective_offset` が
/// `[0, content_height - viewport_height]` へ丸める経路）を一度も経由しないという
/// 問題があった。本型は `measure` が常に `available` に依存しない固定の大きな高さを
/// 返すことで、実際にビューポートを超えるコンテンツを模し、上限クランプを検証可能にする。
#[derive(Debug, Clone)]
struct OverflowingContent {
    id: WidgetId,
    natural_height: f32,
}

impl Widget<FeedState> for OverflowingContent {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn sync_from_state(&mut self, _state: &FeedState) {}

    fn measure(&self, available: Measurement) -> Measurement {
        Measurement {
            width: available.width,
            height: self.natural_height,
        }
    }

    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        vec![PaintCommand::Rect]
    }

    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    fn accessibility_role(&self) -> fandhe_core::AccessibilityRole {
        fandhe_core::AccessibilityRole::Label
    }

    fn interaction_state(&self) -> fandhe_core::InteractionState {
        fandhe_core::InteractionState::default()
    }

    fn clone_box(&self) -> Box<dyn Widget<FeedState>> {
        Box::new(self.clone())
    }
}

/// (4) `set_offset` で設定した生オフセットが `effective_offset` によって
/// `[0, content_height - viewport_height]` へクランプされること（境界値: 負値・
/// NaN・inf に加え、上限側の実オーバーフロー）を、`Widget` トレイトを直接呼び出して
/// 検証する（`scroll::clamp_scroll_offset`、TASK-5.5-S5 / #88 の既存契約と整合）。
#[test]
fn set_offset_clamps_across_boundary_inputs_via_effective_offset() {
    let viewport = Measurement {
        width: 300.0,
        height: 200.0,
    };
    // ビューポート高さ（200.0）を大きく超える固定高さ（1000.0）を返すコンテンツ。
    // `content_height - viewport_height` = 800.0 が上限クランプの想定値になる。
    let tall_content = || OverflowingContent {
        id: WidgetId::from_raw(3),
        natural_height: 1000.0,
    };

    let mut view: ScrollView<FeedState> =
        ScrollView::new(WidgetId::from_raw(4), tall_content()).with_axis(ScrollAxis::Vertical);
    view.set_offset(0.0, -100.0);
    assert_eq!(view.raw_offset(), (0.0, 0.0));

    view.set_offset(0.0, f32::NAN);
    assert_eq!(view.raw_offset(), (0.0, 0.0));

    view.set_offset(0.0, f32::INFINITY);
    assert_eq!(view.raw_offset(), (0.0, 0.0));

    // 下限側: raw_offset が 0.0 のとき effective_offset も 0.0 のまま。
    let (_, offset_y_lower) = view.effective_offset(viewport);
    assert_eq!(offset_y_lower, 0.0);

    // 上限側: コンテンツ高さ（1000.0）がビューポート（200.0）を超えるため、
    // 過大なオフセットは `content_height - viewport_height` = 800.0 にクランプされる。
    view.set_offset(0.0, 100_000.0);
    let (_, offset_y_upper) = view.effective_offset(viewport);
    assert_eq!(offset_y_upper, 800.0);
}

/// (5) `accessibility_role` が想定ロール（`AccessibilityRole::ScrollView`）を返すことを
/// 確認する。
#[test]
fn accessibility_role_is_scroll_view() {
    let view: ScrollView<FeedState> = ScrollView::new(
        WidgetId::from_raw(5),
        Label::fixed(WidgetId::from_raw(6), "フィード"),
    );
    assert_eq!(
        view.accessibility_role(),
        fandhe_core::AccessibilityRole::ScrollView
    );
}

/// `paint` が自前の `Text` を出力せず、子コンテンツ（`Label`）由来の `Text` のみを
/// 含むことを確認する（外部由来文字列の非流出、OWASP A03 対策）。
#[test]
fn paint_delegates_content_paint_without_emitting_own_text() {
    let view: ScrollView<FeedState> = ScrollView::new(
        WidgetId::from_raw(7),
        Label::fixed(WidgetId::from_raw(8), "フィード一覧"),
    );
    let size = Measurement {
        width: 100.0,
        height: 100.0,
    };
    let commands = view.paint((0.0, 0.0), size);

    assert_eq!(commands[0], PaintCommand::Rect);
    assert!(commands.contains(&PaintCommand::Text("フィード一覧".to_string())));
}

/// `Node::Widget` 経由で組み立てたスクロールビューが 1 ノード分正しく追加されることを
/// 確認する（`icon_widget.rs` の `icon_screen_builds_one_widget_node` と同じ観点）。
#[test]
fn scroll_view_screen_builds_one_widget_node() {
    let screen = feed_screen();
    let mut count = 0;
    screen.visit(|node| {
        if let Node::Widget(_) = node {
            count += 1;
        }
    });

    assert_eq!(count, 1);
}
