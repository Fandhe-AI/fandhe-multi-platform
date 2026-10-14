//! `WidgetHarness` の結合テスト（TASK-8.3-S5 / REQ-8、Issue #100）。
//!
//! 設計文書 `docs/architecture/widget-architecture.md` §4.4 の 7 状態ライフサイクル
//! （宣言 → mount → update → layout → paint → event → unmount）と、`ui::ListRecycler`
//! 同様の再利用条件（状態バージョン不変ならスナップショットを再構築しない）を、
//! S7〜S60 のウィジェット結合テストが真似できる使用例として検証する。矩形の算出
//! （レイアウトプロトコル、Issue #99）自体はスコープ外のため、`pointer_dispatch.rs`
//! と同じく `HitRegion` は決め打ちの矩形をテスト側で組み立てる。

use fandhe_core::{
    HitRegion, LifecyclePhase, Point, PointerEvent, PointerEventKind, Rect, Screen, Size,
    WidgetHarness, WidgetId,
};

/// カウンタ状態。ボタン押下で 1 増える。
#[derive(Default)]
struct Counter {
    count: u32,
}

fn increment(state: &mut Counter) {
    state.count += 1;
}

fn button_screen() -> Screen<Counter> {
    Screen::new("home").button(WidgetId::from_raw(1), "increment", increment)
}

/// `increment` ボタンに対応する単一リージョンの矩形ツリー。
fn button_region() -> HitRegion {
    HitRegion::leaf(
        Rect::new(Point::new(0.0, 0.0), Size::new(20.0, 20.0)),
        "increment",
    )
}

/// クリック確定経路: ボタンをタップすると状態のカウンタが増加し、`version` が +1、
/// 次の `run_frame` でスナップショットが更新される。
#[test]
fn tap_confirms_click_and_next_frame_reflects_updated_state() {
    let mut harness = WidgetHarness::new(button_screen(), Counter::default());
    harness.set_regions(button_region());
    harness.run_frame();

    let handled = harness.tap(Point::new(10.0, 10.0));

    assert!(handled);
    assert_eq!(harness.state().count, 1);
    assert_eq!(harness.version(), 1);

    let snapshot = harness.run_frame().to_vec();
    assert_eq!(
        snapshot,
        vec!["# home".to_string(), "(increment)".to_string()]
    );
}

/// press-and-drag-away: ボタン上で Down → 矩形外で Up はクリック不成立となり、
/// 状態・`version` ともに変化しない（S3 の既存挙動との整合確認）。
#[test]
fn press_and_drag_away_does_not_confirm_click() {
    let mut harness = WidgetHarness::new(button_screen(), Counter::default());
    harness.set_regions(button_region());

    let down_handled = harness.pointer(PointerEvent::new(
        PointerEventKind::Down,
        Point::new(5.0, 5.0),
    ));
    let up_handled = harness.pointer(PointerEvent::new(
        PointerEventKind::Up,
        Point::new(500.0, 500.0),
    ));

    assert!(!down_handled);
    assert!(!up_handled);
    assert_eq!(harness.state().count, 0);
    assert_eq!(harness.version(), 0);
}

/// 再利用条件: イベントなしで `run_frame` を 2 回呼ぶと、2 回目は `frames_reused` が
/// 加算され `frames_rendered` は増えない（バージョン不変なら再構築しない、
/// `ui::ListRecycler` と同じ判断軸）。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged() {
    let mut harness = WidgetHarness::new(button_screen(), Counter::default());
    harness.set_regions(button_region());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// ライフサイクル順序: `new` → `set_regions` → `run_frame` → `tap` → `run_frame` →
/// `unmount` の駆動後、`phases()` が設計文書 §4.4 の遷移規則
/// （`Mounted`/`Updated` → `Layout` → `Paint`、`Event` 後に `Updated` へ戻る）へ
/// 整合する順序であることを検証する。
#[test]
fn phases_follow_mount_layout_paint_event_update_layout_paint_unmount_order() {
    let mut harness = WidgetHarness::new(button_screen(), Counter::default());
    harness.set_regions(button_region());
    harness.run_frame();
    harness.tap(Point::new(10.0, 10.0));
    harness.run_frame();
    harness.unmount();

    assert_eq!(
        harness.phases(),
        &[
            LifecyclePhase::Declared,
            LifecyclePhase::Mounted,
            LifecyclePhase::Layout,
            LifecyclePhase::Paint,
            LifecyclePhase::Event,
            LifecyclePhase::Updated,
            LifecyclePhase::Layout,
            LifecyclePhase::Paint,
            LifecyclePhase::Unmounted,
        ]
    );
}

/// unmount 後の駆動拒否: `unmount` 後の `run_frame` / `pointer` / `set_regions` は
/// no-op（または `false`）であり、状態・フェーズ列を変化させない。
#[test]
fn driving_after_unmount_is_a_no_op() {
    let mut harness = WidgetHarness::new(button_screen(), Counter::default());
    harness.set_regions(button_region());
    harness.run_frame();
    harness.unmount();

    let phases_after_unmount = harness.phases().to_vec();
    let snapshot_after_unmount = harness.run_frame().to_vec();
    let tap_handled = harness.tap(Point::new(10.0, 10.0));
    harness.set_regions(button_region());

    assert_eq!(harness.phases(), phases_after_unmount.as_slice());
    assert_eq!(harness.last_snapshot(), snapshot_after_unmount.as_slice());
    assert!(!tap_handled);
    assert_eq!(harness.state().count, 0);
    assert_eq!(harness.version(), 0);
}
