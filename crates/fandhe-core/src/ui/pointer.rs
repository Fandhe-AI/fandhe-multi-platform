//! ポインタイベント（press / release / capture）のディスパッチ基盤（TASK-8.3-S3 / REQ-8）。
//!
//! プラットフォーム統合レイヤ（未実装）がネイティブのタップ・クリック・ドラッグ等を
//! [`PointerEvent`] へ変換し、[`PointerDispatcher`] に流し込む窓口として使う想定。
//! ヒットテスト（[`super::hit_test`]）で座標→ウィジェットキーを解決し、
//! press → release の一連性を検証してクリック確定を判定した上で、
//! 既存の [`crate::state::Event`] へブリッジする（走査ロジックの二重管理を避け、
//! 状態 `S` の実際の書き換えは既存の `StateStore::dispatch` に委譲する。
//! `state::store` モジュール冒頭のコメントで説明されている「`&mut S` を公開 API へ
//! 直接晒さない」という AI 型安全 DSL の方針を、本モジュールでも踏襲している）。
//!
//! ウィジェット共通トレイト（TASK-8.3-S1 / Issue #96）・インタラクション状態遷移
//! （TASK-8.3-S2 / Issue #97）への配線は行わず、本モジュールは `state::Event` を
//! 生成するところまでに留める（`.claude/rules/out-of-scope-tracking.md` に基づく
//! スコープ外の明記）。

use super::geometry::Point;
use super::hit_test::{HitRegion, hit_test};
use crate::state::Event;

/// プラットフォームから受け取るポインタイベントの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEventKind {
    /// ポインタが押下された（タップ開始・マウスボタン押下）。
    Down,
    /// ポインタが離された（タップ終了・マウスボタン解放）。
    Up,
    /// ポインタが移動した（本 issue のスコープではキャプチャ状態を変更しない）。
    Move,
    /// OS 側のジェスチャ横取り等によるキャンセル。キャプチャを解放するのみ。
    Cancel,
}

/// 1 件のポインタイベント（種別と発生座標）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerEvent {
    /// イベント種別。
    pub kind: PointerEventKind,
    /// 発生座標（レイアウト済み矩形ツリーと同じ座標系）。
    pub position: Point,
}

impl PointerEvent {
    /// 指定種別・座標のポインタイベントを作る。
    pub fn new(kind: PointerEventKind, position: Point) -> Self {
        Self { kind, position }
    }
}

/// ヒットテスト結果からポインタイベントを解釈し、クリック確定を判定するディスパッチャ。
///
/// 内部状態は「キャプチャ中のウィジェットキー」のみ。Down で最前面のキー付きターゲットを
/// キャプチャし、そのキャプチャ対象上での Up をクリック確定として扱う（キャプチャ機構により
/// 「押下開始位置と異なるウィジェット上での Up」を誤ってクリック扱いしない、一般的な
/// GUI のポインタジェスチャ規約）。
#[derive(Debug, Default)]
pub struct PointerDispatcher {
    captured_key: Option<String>,
}

impl PointerDispatcher {
    /// キャプチャなしの初期状態でディスパッチャを作る。
    pub fn new() -> Self {
        Self::default()
    }

    /// 現在キャプチャ中のウィジェットキー（デバッグ・テスト用途）。
    pub fn captured_key(&self) -> Option<&str> {
        self.captured_key.as_deref()
    }

    /// `root` を対象に `event` を処理し、クリックが確定した場合は対応する
    /// `state::Event` を返す。
    ///
    /// - `Down`: `root` に対するヒットテストで最前面のキー付きターゲットをキャプチャする。
    ///   ヒットなしの場合はキャプチャを持たない状態にする（`None` を返す）。
    /// - `Up`: キャプチャ中のキーが、現在の座標でのヒットテスト結果に含まれていれば
    ///   （最前面である必要はない。キャプチャ中ウィジェットの手前に重なる／ネストした
    ///   別のキー付きウィジェットが最前面に来ていても、キャプチャ対象自体がまだ
    ///   ポインタ直下にあればクリック確定として扱う）クリック確定として
    ///   `Event::ButtonClicked` を返し、キャプチャを解放する。
    ///   キャプチャ中のキーがヒットテスト結果に含まれない・キャプチャなしの場合は
    ///   `None` を返し、キャプチャは解放する（press-and-drag-away で完全に外れて
    ///   離した場合にのみ発火しない一般的な挙動）。
    /// - `Move`: キャプチャ状態を変更しない（本 issue のスコープでは hover 等の
    ///   インタラクション状態遷移は扱わない。TASK-8.3-S2 / Issue #97 のスコープ）。
    /// - `Cancel`: キャプチャを解放するのみ。
    ///
    /// いずれの分岐も panic せず `None` を返せる設計にしている
    /// （キャプチャ中ターゲット消失・Down なしの Up 等の異常系、coding-rust.md 準拠）。
    pub fn dispatch(&mut self, root: &HitRegion, event: PointerEvent) -> Option<Event> {
        match event.kind {
            PointerEventKind::Down => {
                let hits = hit_test(root, event.position);
                self.captured_key = hits.into_iter().next().map(|target| target.key);
                None
            }
            PointerEventKind::Up => {
                let captured = self.captured_key.take()?;
                let hits = hit_test(root, event.position);
                if hits.iter().any(|target| target.key == captured) {
                    Some(Event::ButtonClicked { label: captured })
                } else {
                    None
                }
            }
            PointerEventKind::Move => None,
            PointerEventKind::Cancel => {
                self.captured_key = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::geometry::{Point, Rect, Size};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(Point::new(x, y), Size::new(w, h))
    }

    fn button_region() -> HitRegion {
        HitRegion::leaf(rect(0.0, 0.0, 10.0, 10.0), "increment")
    }

    #[test]
    fn down_then_up_on_same_target_confirms_click() {
        let region = button_region();
        let mut dispatcher = PointerDispatcher::new();

        let down = dispatcher.dispatch(
            &region,
            PointerEvent::new(PointerEventKind::Down, Point::new(5.0, 5.0)),
        );
        assert!(down.is_none());
        assert_eq!(dispatcher.captured_key(), Some("increment"));

        let up = dispatcher.dispatch(
            &region,
            PointerEvent::new(PointerEventKind::Up, Point::new(5.0, 5.0)),
        );

        assert_eq!(
            up,
            Some(Event::ButtonClicked {
                label: "increment".to_string()
            })
        );
        assert_eq!(dispatcher.captured_key(), None);
    }

    #[test]
    fn down_then_up_outside_target_does_not_fire() {
        let region = button_region();
        let mut dispatcher = PointerDispatcher::new();

        dispatcher.dispatch(
            &region,
            PointerEvent::new(PointerEventKind::Down, Point::new(5.0, 5.0)),
        );
        let up = dispatcher.dispatch(
            &region,
            PointerEvent::new(PointerEventKind::Up, Point::new(50.0, 50.0)),
        );

        assert!(up.is_none());
        assert_eq!(dispatcher.captured_key(), None);
    }

    #[test]
    fn up_without_prior_down_does_not_fire() {
        let region = button_region();
        let mut dispatcher = PointerDispatcher::new();

        let up = dispatcher.dispatch(
            &region,
            PointerEvent::new(PointerEventKind::Up, Point::new(5.0, 5.0)),
        );

        assert!(up.is_none());
    }

    #[test]
    fn cancel_releases_capture() {
        let region = button_region();
        let mut dispatcher = PointerDispatcher::new();

        dispatcher.dispatch(
            &region,
            PointerEvent::new(PointerEventKind::Down, Point::new(5.0, 5.0)),
        );
        assert_eq!(dispatcher.captured_key(), Some("increment"));

        let cancel = dispatcher.dispatch(
            &region,
            PointerEvent::new(PointerEventKind::Cancel, Point::new(5.0, 5.0)),
        );

        assert!(cancel.is_none());
        assert_eq!(dispatcher.captured_key(), None);
    }

    #[test]
    fn up_on_captured_target_confirms_click_even_when_overlapping_sibling_is_frontmost() {
        // captured 対象の手前に重なる別のキー付きウィジェット（front）が存在する場合でも、
        // captured 自体がまだヒットテスト結果に含まれていればクリックを確定させる
        // （Cursor Bugbot 指摘: 最前面ヒットのみで判定すると、キャプチャ中ウィジェット上に
        // 重なる／ネストした別ウィジェットが最前面になった際に press-and-drag-away 以外の
        // 正当なクリックまで取りこぼす）。
        let captured_region = HitRegion::leaf(rect(0.0, 0.0, 10.0, 10.0), "increment");
        let overlay = HitRegion::leaf(rect(0.0, 0.0, 10.0, 10.0), "overlay");
        let root = HitRegion::container(rect(0.0, 0.0, 10.0, 10.0), vec![captured_region, overlay]);
        let mut dispatcher = PointerDispatcher::new();

        // Down 時点では overlay がまだ無く、captured_region のみがヒットする状況を
        // 素朴な 1 リージョンツリーで表現し、captured_key に "increment" を設定する。
        let down_only_region = HitRegion::leaf(rect(0.0, 0.0, 10.0, 10.0), "increment");
        let down = dispatcher.dispatch(
            &down_only_region,
            PointerEvent::new(PointerEventKind::Down, Point::new(5.0, 5.0)),
        );
        assert!(down.is_none());
        assert_eq!(dispatcher.captured_key(), Some("increment"));

        // Up 時点では overlay が最前面としてヒットするが、captured の "increment" も
        // 依然としてヒットテスト結果に含まれているため、クリックは確定する。
        let up = dispatcher.dispatch(
            &root,
            PointerEvent::new(PointerEventKind::Up, Point::new(5.0, 5.0)),
        );

        assert_eq!(
            up,
            Some(Event::ButtonClicked {
                label: "increment".to_string()
            })
        );
        assert_eq!(dispatcher.captured_key(), None);
    }

    #[test]
    fn down_outside_any_target_clears_capture() {
        let region = button_region();
        let mut dispatcher = PointerDispatcher::new();

        dispatcher.dispatch(
            &region,
            PointerEvent::new(PointerEventKind::Down, Point::new(5.0, 5.0)),
        );
        dispatcher.dispatch(
            &region,
            PointerEvent::new(PointerEventKind::Down, Point::new(50.0, 50.0)),
        );

        assert_eq!(dispatcher.captured_key(), None);
    }
}
