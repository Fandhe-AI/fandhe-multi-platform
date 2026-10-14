//! レイアウト専用の最小ジオメトリ型。
//!
//! `ui::layout` モジュール内で完結させる公開型で、taffy の座標・サイズ型を
//! `fandhe-core` の公開 API へ直接露出させないための変換先として使う
//! （TASK-8.3-S4 / REQ-8、Issue #99）。
//!
//! `ui::geometry::{Point, Size, Rect}`（イベントディスパッチ・ヒットテスト、Issue #98）
//! との名前衝突を避けるため `Layout` 接頭辞を付けている。型統合は
//! 後続作業とする（out-of-scope-tracking.md 対象）。

/// レイアウト計算結果・入力で使う 2 次元サイズ（幅・高さ、単位はポイント）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutSize {
    /// 幅（ポイント）。
    pub width: f32,
    /// 高さ（ポイント）。
    pub height: f32,
}

impl LayoutSize {
    /// 幅・高さともに 0 のサイズ。
    pub const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
    };

    /// 幅・高さを指定してサイズを作る。
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// レイアウト計算結果で使う 2 次元座標（親ノード相対、単位はポイント）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutPoint {
    /// X 座標。
    pub x: f32,
    /// Y 座標。
    pub y: f32,
}

impl LayoutPoint {
    /// 原点（0, 0）。
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    /// X・Y を指定して座標を作る。
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// レイアウト計算後の 1 ノード分の配置結果（親ノード相対の原点 + サイズ）。
///
/// `LayoutTree::layout` が返す型で、呼び出し元（プラットフォームレイヤの描画走査、
/// 本 issue のスコープ外）はこの値を親からの相対オフセットとして積み上げて
/// 画面座標へ変換する想定。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutRect {
    /// 親ノード相対の原点。
    pub origin: LayoutPoint,
    /// ノードのサイズ。
    pub size: LayoutSize,
}

impl LayoutRect {
    /// 原点とサイズを指定して矩形を作る。
    pub const fn new(origin: LayoutPoint, size: LayoutSize) -> Self {
        Self { origin, size }
    }
}
