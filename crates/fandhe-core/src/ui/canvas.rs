//! `Node::Widget` へ差し込む汎用キャンバスウィジェット [`CanvasWidget`]
//! （Issue #830 の型サンプル `tilt_ball` が最初の利用者）。
//!
//! `PaintCommand`（`Circle`/`StyledRect` 等）を実描画へ届ける経路が本クレート内に
//! 存在しなかった（`fandhe-egui-proxy::paint_command` の変換関数はあるが
//! `Node::Widget` からの実配線が無い、Issue #828 の同モジュール doc 参照）ため、
//! 本ウィジェットが橋渡しを担う。`ui/progress.rs`（`ProgressBar<S>`）と同じ構造上の
//! 判断（`Widget<S>` を実装する自己完結の最小型、状態の SSOT はアプリ状態 `S`）を踏襲する。
//!
//! egui-proxy 側は `Checkbox`/`Slider` 等（Issue #806）と同じ `clone_box` →
//! `sync_from_state` → `as_any().downcast_ref` の経路で `CanvasWidget<S>` を見つけ、
//! [`Widget::paint`] が返す `PaintCommand` 列を `paint_command::paint_circle` 等へ渡す
//! （`fandhe-egui-proxy::lib.rs` の `Node::Widget` 分岐参照。`Widget::paint()` を
//! 実際に呼ぶのは egui-proxy のみで、`render_headless` / `fandhe-web-dom` /
//! `fandhe-swiftui-bridge` は `Node::Widget` を状態非依存のプレースホルダとして扱う
//! 既存契約を維持する、`widget.rs` の「未配線の契約」参照）。
//!
//! 個別画面固有の型（玉の物理等）は持たず、`paint: ReadFn<S, Vec<PaintCommand>>` 経由で
//! 画面側（`screens::tilt_ball` 等）が描画内容を組み立てる（プラットフォームレイヤが
//! `screens::` の個別型を直接掴まないようにする設計判断、Issue #830）。
//! `paint` が返す `PaintCommand` の座標はキャンバス左上を原点とするローカル座標とし、
//! [`Widget::paint`] がキャンバスの配置矩形 `origin` 分だけ平行移動してから返す
//! （画面側の物理計算をキャンバスの画面上位置から独立させるため）。

use super::widget::{AccessibilityRole, InteractionState, Measurement, Widget, WidgetId};
use crate::PaintCommand;
use crate::state::ReadFn;

/// アプリ状態 `S` から `PaintCommand` 列を読み取り、確定した配置矩形へ合わせて
/// 平行移動した上で描画へ渡す汎用ウィジェット。
pub struct CanvasWidget<S> {
    id: WidgetId,
    size: Measurement,
    label: String,
    /// アプリ状態 `S` から描画コマンド列（キャンバス左上原点のローカル座標）を
    /// 読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    paint: ReadFn<S, Vec<PaintCommand>>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（[`Widget::paint`] が
    /// 平行移動して返す元データ）。
    cached: Vec<PaintCommand>,
    interaction: InteractionState,
    /// true のときだけ描画層がキャンバス上のドラッグを捕捉する（ペン描画用）。
    /// 既定 false では親 `ScrollArea` のドラッグスクロールを妨げない。
    captures_drag: bool,
}

impl<S> std::fmt::Debug for CanvasWidget<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanvasWidget")
            .field("id", &self.id)
            .field("size", &self.size)
            .field("label", &self.label)
            .field("cached", &self.cached)
            .field("interaction", &self.interaction)
            .field("captures_drag", &self.captures_drag)
            .finish()
    }
}

impl<S> Clone for CanvasWidget<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            size: self.size,
            label: self.label.clone(),
            paint: self.paint,
            cached: self.cached.clone(),
            interaction: self.interaction,
            captures_drag: self.captures_drag,
        }
    }
}

impl<S> CanvasWidget<S> {
    /// 状態 `S` に束縛したキャンバスを作る（ビルダー形式の起点）。
    ///
    /// `paint` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`]）と代入互換の
    /// 関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで拒否する）。
    pub fn bound(
        id: WidgetId,
        label: impl Into<String>,
        size: Measurement,
        paint: ReadFn<S, Vec<PaintCommand>>,
    ) -> Self {
        Self {
            id,
            size,
            label: label.into(),
            paint,
            cached: Vec::new(),
            interaction: InteractionState::default(),
            captures_drag: false,
        }
    }

    /// キャンバス上のドラッグを描画層で捕捉する（ペン描画画面のみ。親のドラッグスクロールより優先）。
    pub fn with_drag_capture(mut self) -> Self {
        self.captures_drag = true;
        self
    }

    /// ドラッグを捕捉するか（描画層が `Sense` を決める入力）。
    pub fn captures_drag(&self) -> bool {
        self.captures_drag
    }
}

/// `command` を `(dx, dy)` だけ平行移動する。`Circle` の中心座標のみが位置を持ち、
/// `InsetRect` は辺からの距離で位置を持たないため同じく素通しする。
/// それ以外のバリアントはキャンバスの配置矩形全体を暗黙の境界とする契約
/// （`StyledRect` 等、既存ウィジェットの `PaintCommand` の使われ方と同じ）ため素通しする。
fn translate(command: PaintCommand, dx: f32, dy: f32) -> PaintCommand {
    match command {
        PaintCommand::Circle {
            center: (x, y),
            radius,
            fill,
        } => PaintCommand::Circle {
            center: (x + dx, y + dy),
            radius,
            fill,
        },
        other => other,
    }
}

impl<S: 'static> Widget<S> for CanvasWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から `PaintCommand` 列を読み取り `cached` を更新する
    /// （reconcile フェーズ）。未配線の契約は `widget.rs` の `sync_from_state` doc 参照。
    fn sync_from_state(&mut self, state: &S) {
        self.cached = (self.paint)(state);
    }

    /// 構築時に固定したサイズを `available` でクランプして返す。
    fn measure(&self, available: Measurement) -> Measurement {
        Measurement {
            width: self.size.width.min(available.width),
            height: self.size.height.min(available.height),
        }
    }

    /// `cached` を配置矩形の `origin` 分だけ平行移動して返す（モジュール doc 参照）。
    fn paint(&self, origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        self.cached
            .iter()
            .cloned()
            .map(|c| translate(c, origin.0, origin.1))
            .collect()
    }

    /// 配置矩形内包のみで判定する（クリック等の個別処理は持たない表示専用ウィジェット、
    /// `ProgressBar` と同じ判断）。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        point.0 >= origin.0
            && point.0 <= origin.0 + size.width
            && point.1 >= origin.1
            && point.1 <= origin.1 + size.height
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Image
    }

    fn accessibility_label(&self) -> Option<String> {
        if self.label.is_empty() {
            None
        } else {
            Some(self.label.clone())
        }
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::ColorValue;

    struct DummyState {
        cx: f32,
    }

    fn commands(state: &DummyState) -> Vec<PaintCommand> {
        vec![
            PaintCommand::StyledRect {
                fill: ColorValue::rgba(0, 0, 0, 255),
            },
            PaintCommand::Circle {
                center: (state.cx, 5.0),
                radius: 3.0,
                fill: ColorValue::rgba(255, 0, 0, 255),
            },
        ]
    }

    fn size() -> Measurement {
        Measurement {
            width: 100.0,
            height: 100.0,
        }
    }

    #[test]
    fn paint_before_sync_is_empty() {
        let widget = CanvasWidget::bound(WidgetId::from_raw(1), "盤面", size(), commands);
        assert!(widget.paint((0.0, 0.0), size()).is_empty());
    }

    #[test]
    fn paint_after_sync_translates_circle_by_origin() {
        let mut widget = CanvasWidget::bound(WidgetId::from_raw(1), "盤面", size(), commands);
        widget.sync_from_state(&DummyState { cx: 10.0 });

        let painted = widget.paint((50.0, 20.0), size());
        assert_eq!(painted.len(), 2);
        assert!(matches!(painted[0], PaintCommand::StyledRect { .. }));
        match &painted[1] {
            PaintCommand::Circle { center, .. } => assert_eq!(*center, (60.0, 25.0)),
            other => panic!("Circle を期待したが {other:?} だった"),
        }
    }

    #[test]
    fn measure_clamps_to_available() {
        let widget = CanvasWidget::bound(WidgetId::from_raw(1), "盤面", size(), commands);
        let measured = widget.measure(Measurement {
            width: 40.0,
            height: 200.0,
        });
        assert_eq!(measured.width, 40.0);
        assert_eq!(measured.height, 100.0);
    }

    #[test]
    fn hit_test_checks_bounding_rect_only() {
        let widget = CanvasWidget::bound(WidgetId::from_raw(1), "盤面", size(), commands);
        assert!(widget.hit_test((10.0, 10.0), (0.0, 0.0), size()));
        assert!(!widget.hit_test((-1.0, 10.0), (0.0, 0.0), size()));
    }

    #[test]
    fn accessibility_label_is_none_when_empty() {
        let widget = CanvasWidget::bound(WidgetId::from_raw(1), "", size(), commands);
        assert_eq!(widget.accessibility_label(), None);
        assert_eq!(widget.accessibility_role(), AccessibilityRole::Image);
    }
}
