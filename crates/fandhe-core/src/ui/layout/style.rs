//! 宣言層へ見せるレイアウトスタイル語彙。
//!
//! taffy の `Style` は CSS 相当の広い語彙を持つが、本 issue（TASK-8.3-S4）は
//! 4h スコープの最小サブセットのみを公開する（設計文書 §1 分担表）。フレックス方向・
//! ギャップ・パディング・flex_grow・固定/自動サイズのみを扱い、グリッド等は
//! S43〜S45（スタック・グリッド）で拡張する想定（out-of-scope-tracking.md 対象）。
//!
//! taffy の型は `ui::layout` の外へ露出させない方針（§4.3）のため、本モジュールが
//! `LayoutStyle` → `taffy::style::Style` の変換を一手に引き受ける。
//!
//! TASK-8.4-S15（REQ-8 / REQ-2、#171）で `crate::token`（デザイントークン基盤、
//! TASK-8.4）が確定した寸法トークン（`SpacingValue` / `SizeValue`）との接続を
//! 追加した（[`Dimension::from_size`] / [`LayoutStyle::with_gap_token`] /
//! [`LayoutStyle::with_padding_token`]）。トークン型ビルダーが受け取るのは
//! `TokenResolver::resolve` 済みの具体値であり、`LayoutStyle` 自身は
//! `TokenRef`（未解決の参照）を一切保持しない。taffy へ渡す `to_taffy` の
//! 変換境界（下記）は本 issue のスコープ外で、トークン経由か生 `f32` 経由かを
//! 問わず同じ経路を通る。トークン値は構築時点（`SpacingValue::new` /
//! `DimensionValue::new`）で有限・非負が検証済みのため、`to_taffy` の
//! `.max(0.0)` クランプは実質的に到達しない防御の多重化に留まる
//! （生 `f32` 経由のビルダーが検証を経ない値を渡す既存経路のために残す）。

/// 幅・高さに指定できる値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dimension {
    /// レイアウトエンジンが内容・親の制約から自動計算する。
    Auto,
    /// 固定ポイント数。
    Points(f32),
}

/// フレックスコンテナの主軸方向。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FlexDirection {
    /// 横方向（左→右）に子を並べる。
    Row,
    /// 縦方向（上→下）に子を並べる。
    #[default]
    Column,
}

/// レイアウトノード 1 つ分のスタイル（コンテナ・リーフ共通で使う）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutStyle {
    /// 幅。
    pub width: Dimension,
    /// 高さ。
    pub height: Dimension,
    /// 子の並び方向（このノードがコンテナの場合のみ意味を持つ）。
    pub direction: FlexDirection,
    /// 子同士の間隔（ポイント）。
    pub gap: f32,
    /// 四辺均等パディング（ポイント）。
    pub padding: f32,
    /// 余剰スペースの分配率。0.0 は分配を受けない（taffy 既定と同じ）。
    pub flex_grow: f32,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        Self {
            width: Dimension::Auto,
            height: Dimension::Auto,
            direction: FlexDirection::default(),
            gap: 0.0,
            padding: 0.0,
            flex_grow: 0.0,
        }
    }
}

impl Dimension {
    fn to_taffy(self) -> taffy::style::Dimension {
        match self {
            Self::Auto => taffy::style::Dimension::auto(),
            Self::Points(value) => taffy::style::Dimension::length(value),
        }
    }

    /// サイズトークン（[`crate::token::SizeValue`]）から固定ポイント数の
    /// `Dimension` を構築する（TASK-8.4-S15、#171）。`SizeValue::value()` は
    /// 構築時点で有限・非負が検証済みのため、そのまま [`Self::Points`] へ渡す。
    pub fn from_size(value: crate::token::SizeValue) -> Self {
        Self::Points(value.value())
    }
}

impl From<FlexDirection> for taffy::style::FlexDirection {
    fn from(value: FlexDirection) -> Self {
        match value {
            FlexDirection::Row => Self::Row,
            FlexDirection::Column => Self::Column,
        }
    }
}

impl LayoutStyle {
    /// `gap` をスペーシングトークン（[`crate::token::SpacingValue`]）から設定する
    /// ビルダーメソッド（TASK-8.4-S15、#171）。`SpacingValue::value()` は構築時点
    /// で有限・非負が検証済みのため、生 `f32` 版（[`Self::gap`] フィールドの直接
    /// 代入）と異なり追加の正規化は行わない。
    pub fn with_gap_token(mut self, value: crate::token::SpacingValue) -> Self {
        self.gap = value.value();
        self
    }

    /// `padding` をスペーシングトークンから設定するビルダーメソッド（同上）。
    pub fn with_padding_token(mut self, value: crate::token::SpacingValue) -> Self {
        self.padding = value.value();
        self
    }

    /// `fandhe-core` 所有のスタイル語彙を taffy の内部表現へ変換する。
    ///
    /// `ui::layout::tree` からのみ呼ばれる内部変換であり、`taffy::style::Style` を
    /// `ui::layout` の外へ渡さない（公開 API 非露出の方針、§4.3）。
    pub(super) fn to_taffy(self) -> taffy::style::Style {
        let padding = taffy::style::LengthPercentage::length(self.padding.max(0.0));
        let gap = taffy::style::LengthPercentage::length(self.gap.max(0.0));
        taffy::style::Style {
            display: taffy::style::Display::Flex,
            flex_direction: self.direction.into(),
            gap: taffy::geometry::Size {
                width: gap,
                height: gap,
            },
            padding: taffy::geometry::Rect {
                left: padding,
                right: padding,
                top: padding,
                bottom: padding,
            },
            flex_grow: self.flex_grow.max(0.0),
            size: taffy::geometry::Size {
                width: self.width.to_taffy(),
                height: self.height.to_taffy(),
            },
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{DimensionValue, SpacingValue};

    #[test]
    fn dimension_from_size_yields_points_with_same_value() {
        let size = DimensionValue::new(48.0).expect("正当な値");
        assert_eq!(Dimension::from_size(size), Dimension::Points(48.0));
    }

    #[test]
    fn with_gap_token_matches_raw_f32_equivalent() {
        let spacing = SpacingValue::new(16.0).expect("正当な値");
        let from_token = LayoutStyle::default().with_gap_token(spacing);
        let from_raw = LayoutStyle {
            gap: 16.0,
            ..Default::default()
        };
        assert_eq!(from_token, from_raw);
    }

    #[test]
    fn with_padding_token_matches_raw_f32_equivalent() {
        let spacing = SpacingValue::new(8.0).expect("正当な値");
        let from_token = LayoutStyle::default().with_padding_token(spacing);
        let from_raw = LayoutStyle {
            padding: 8.0,
            ..Default::default()
        };
        assert_eq!(from_token, from_raw);
    }

    #[test]
    fn with_gap_token_and_padding_token_are_reflected_in_taffy_conversion() {
        // `LayoutTree` を経由した制約解決の統合検証は `layout::tree` の
        // テストパターン（`gap`/`padding` を与えた `compute_layout` 結果検証）を
        // 踏襲する。ここでは `ui::layout` の外へ taffy 型を渡さない方針（§4.3）
        // のため `to_taffy` は `pub(super)` のままとし、変換結果は `tree` 経由の
        // 統合テスト（`tree.rs`）で検証する。本テストはトークン版ビルダーが
        // taffy 変換に到達可能な状態（`LayoutStyle` フィールドへの反映）まで
        // 検証する。
        let spacing = SpacingValue::new(12.0).expect("正当な値");
        let style = LayoutStyle::default()
            .with_gap_token(spacing)
            .with_padding_token(spacing);
        assert_eq!(style.gap, 12.0);
        assert_eq!(style.padding, 12.0);
    }
}
