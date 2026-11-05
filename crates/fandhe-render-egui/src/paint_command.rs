//! `fandhe_core::PaintCommand::Circle`（#828）を egui の描画プリミティブへ変換する
//! プラットフォーム統合レイヤ。
//!
//! 本モジュールは純粋変換のみを担い、`Node::Widget` 分岐からの実配線は行わない
//! （`lib.rs` の `render_node` は `PaintCommand` を消費する経路をまだ持たない。
//! `ui/render.rs` の `Widget::paint()` 未配線の注記と同じ理由、Issue #98）。
//! 実際に画面へ配線する型サンプルは #830（`tilt_ball`）が担う。

use egui::{Color32, Painter, Shape};
use fandhe_core::PaintCommand;

/// `PaintCommand::Circle` を egui の塗りつぶし円 `Shape` へ変換する。
///
/// `Circle` 以外のバリアントは `None` を返す（呼び出し元は他バリアント用の
/// 変換へフォールバックする契約、`fandhe-core` 側の `PaintCommand` doc 参照）。
pub fn circle_shape(command: &PaintCommand) -> Option<Shape> {
    match command {
        PaintCommand::Circle {
            center,
            radius,
            fill,
        } => Some(Shape::circle_filled(
            egui::pos2(center.0, center.1),
            *radius,
            Color32::from_rgba_unmultiplied(fill.r, fill.g, fill.b, fill.a),
        )),
        _ => None,
    }
}

/// `circle_shape` の変換結果を `egui::Painter` へ実際に描画する薄いラッパー。
///
/// `Circle` 以外のコマンドを渡した場合は何もしない（呼び出し元は事前に
/// バリアントを絞り込む必要がない）。
pub fn paint_circle(painter: &Painter, command: &PaintCommand) {
    if let Some(shape) = circle_shape(command) {
        painter.add(shape);
    }
}

/// `PaintCommand::InsetRect` の矩形を求める。`outer` の 4 辺から指定距離だけ内側へ寄せ、
/// 距離が大きすぎて反転する軸は幅・高さ 0 に潰す（min > max にしない、Issue #444）。
pub fn inset_rect(outer: egui::Rect, left: f32, top: f32, right: f32, bottom: f32) -> egui::Rect {
    let min = outer.min + egui::vec2(left, top);
    let max = outer.max - egui::vec2(right, bottom);
    egui::Rect::from_min_max(min, egui::pos2(max.x.max(min.x), max.y.max(min.y)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::token::ColorValue;

    #[test]
    fn inset_rect_moves_edges_and_collapses_when_over() {
        let outer = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 50.0));
        let r = inset_rect(outer, 1.0, 2.0, 3.0, 4.0);
        assert_eq!(r.min, egui::pos2(11.0, 12.0));
        assert_eq!(r.max, egui::pos2(107.0, 56.0));
        let c = inset_rect(outer, 80.0, 0.0, 80.0, 60.0);
        assert_eq!(c.width(), 0.0);
        assert_eq!(c.height(), 0.0);
    }

    #[test]
    fn circle_shape_converts_center_radius_and_color() {
        let command = PaintCommand::Circle {
            center: (10.0, 20.0),
            radius: 5.0,
            fill: ColorValue::rgba(255, 0, 0, 255),
        };

        let shape = circle_shape(&command).expect("Circle は Some を返す契約");
        match shape {
            Shape::Circle(circle) => {
                assert_eq!(circle.center, egui::pos2(10.0, 20.0));
                assert_eq!(circle.radius, 5.0);
                assert_eq!(circle.fill, Color32::from_rgba_unmultiplied(255, 0, 0, 255));
            }
            other => panic!("Shape::Circle を期待したが {other:?} だった"),
        }
    }

    #[test]
    fn circle_shape_returns_none_for_non_circle_commands() {
        assert!(circle_shape(&PaintCommand::Rect).is_none());
        assert!(circle_shape(&PaintCommand::Text("x".to_string())).is_none());
    }
}
