//! 座標・矩形の最小ジオメトリ型（TASK-8.3-S3 / REQ-8）。
//!
//! ヒットテスト（[`super::hit_test`]）とポインタディスパッチ（[`super::pointer`]）が
//! 共有する座標表現。レイアウト側の矩形算出（[`super::layout`]、TASK-8.3-S4 /
//! Issue #99）には踏み込まず、本モジュールで `Point` / `Size` / `Rect` を最小定義する
//! （型名の重複定義を避け、レイアウト結果の矩形表現との統合を単純な構造体のまま
//! 吸収できるようにするため）。

/// 平面上の 1 点（左上原点、下方向が正）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// 横方向の座標。
    pub x: f32,
    /// 縦方向の座標。
    pub y: f32,
}

impl Point {
    /// 指定座標の点を作る。
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// 矩形の幅・高さ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    /// 幅。
    pub width: f32,
    /// 高さ。
    pub height: f32,
}

impl Size {
    /// 指定サイズを作る。
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// 左上原点 (`origin`) とサイズ (`size`) で表す軸並行矩形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// 矩形の左上頂点。
    pub origin: Point,
    /// 矩形の幅・高さ。
    pub size: Size,
}

impl Rect {
    /// 左上頂点とサイズから矩形を作る。
    pub fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    /// `point` が本矩形に含まれるかを判定する。
    ///
    /// 左上を含み右下を含まない半開区間（`[origin, origin + size)`）で判定する。
    /// 幅・高さが負、または `point` / 矩形の値に NaN が含まれる場合は panic せず
    /// `false` を返す（panic を API 契約にしない、coding-rust.md のエラーハンドリング方針。
    /// NaN を含む比較は IEEE 754 の仕様上すべて `false` になるため、比較演算子の並びだけで
    /// 安全に判定できる）。
    pub fn contains(&self, point: Point) -> bool {
        if self.size.width < 0.0 || self.size.height < 0.0 {
            return false;
        }
        let right = self.origin.x + self.size.width;
        let bottom = self.origin.y + self.size.height;
        point.x >= self.origin.x && point.x < right && point.y >= self.origin.y && point.y < bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::new(Point::new(10.0, 10.0), Size::new(20.0, 20.0))
    }

    #[test]
    fn contains_true_for_interior_point() {
        assert!(rect().contains(Point::new(15.0, 15.0)));
    }

    #[test]
    fn contains_true_on_top_left_edge() {
        assert!(rect().contains(Point::new(10.0, 10.0)));
    }

    #[test]
    fn contains_false_on_bottom_right_edge() {
        // 半開区間のため右下端（origin + size ちょうど）は含まない。
        assert!(!rect().contains(Point::new(30.0, 30.0)));
    }

    #[test]
    fn contains_false_outside() {
        assert!(!rect().contains(Point::new(0.0, 0.0)));
        assert!(!rect().contains(Point::new(31.0, 15.0)));
    }

    #[test]
    fn contains_false_for_nan_point() {
        assert!(!rect().contains(Point::new(f32::NAN, 15.0)));
        assert!(!rect().contains(Point::new(15.0, f32::NAN)));
    }

    #[test]
    fn contains_false_for_negative_size() {
        let negative = Rect::new(Point::new(0.0, 0.0), Size::new(-10.0, 10.0));
        assert!(!negative.contains(Point::new(-5.0, 5.0)));
    }

    #[test]
    fn contains_false_for_zero_size() {
        let zero = Rect::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0));
        assert!(!zero.contains(Point::new(0.0, 0.0)));
    }
}
