//! レイアウトツリー構築・制約解決（設計文書 §5 の「`Widget<S>::measure` の呼び出し元
//! ・レイアウトツリー構築・制約解決を実装する」契約の実体）。
//!
//! taffy の `TaffyTree` を内部実装として使い、`fandhe-core` 所有の型（[`LayoutStyle`] /
//! [`LayoutRect`] / [`Measure`]）のみを公開する。taffy の型（`NodeId` / `Style` /
//! `TaffyError` 等）は本モジュールの外へ渡さない（§4.3、taffy 導入判断の前提）。

use std::collections::HashSet;
use std::fmt;

use super::error::LayoutError;
use super::geometry::{LayoutPoint, LayoutRect, LayoutSize};
use super::measure::{AvailableSpace, LayoutConstraints, Measure};
use super::style::LayoutStyle;

/// [`LayoutTree`] 内のノードを指すハンドル。
///
/// taffy の `NodeId` をラップする不透明な値で、生成元の `LayoutTree` 以外に渡すと
/// [`LayoutError::InvalidNode`] を返す（taffy 側の SlotMap 素引きは他ツリー由来の
/// `NodeId` を渡すと panic し得るため、検証は taffy に委ねず `LayoutTree` 自身が行う。
/// 詳細は [`LayoutTree`] の `own_nodes` フィールドのコメントを参照）。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutNodeId(taffy::NodeId);

impl fmt::Debug for LayoutNodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("LayoutNodeId").field(&self.0).finish()
    }
}

/// サイズ計測・配置を行うレイアウトツリー。
///
/// 呼び出し元（プラットフォームレイヤの描画走査・将来の `Widget<S>` 実装層、いずれも
/// 本 issue のスコープ外）は `new_leaf` / `new_leaf_with_measure` / `new_with_children` で
/// ツリーを構築し、`compute_layout` で計算した後 `layout` で各ノードの配置結果を読む。
pub struct LayoutTree {
    inner: taffy::TaffyTree<Box<dyn Measure>>,
    // taffy 0.12 の `TaffyTree::layout`/`set_style` 等は内部の SlotMap を素引きしており、
    // 他ツリー由来の `NodeId` を渡すと `Result` ではなく panic で落ちる（SlotMap の世代
    // 一致・スロット再利用の巡り合わせ次第で誤ったノードを返す可能性もある）。
    // panic を API 契約にしない方針（coding-rust.md）のため、本ツリーが払い出した
    // `NodeId` をここで追跡し、taffy へ委譲する前に自前で存在検証する。
    own_nodes: HashSet<taffy::NodeId>,
}

impl LayoutTree {
    /// 空のレイアウトツリーを作る。
    pub fn new() -> Self {
        Self {
            inner: taffy::TaffyTree::new(),
            own_nodes: HashSet::new(),
        }
    }

    /// 計測を持たないリーフノード（固定サイズが決まっている要素）を追加する。
    pub fn new_leaf(&mut self, style: LayoutStyle) -> Result<LayoutNodeId, LayoutError> {
        let id = self.inner.new_leaf(style.to_taffy())?;
        self.own_nodes.insert(id);
        Ok(LayoutNodeId(id))
    }

    /// [`Measure`] 実装を持つリーフノード（テキスト等、内容に応じてサイズが決まる要素）を
    /// 追加する。`measure` は `compute_layout` の内部から呼び出される。
    pub fn new_leaf_with_measure(
        &mut self,
        style: LayoutStyle,
        measure: Box<dyn Measure>,
    ) -> Result<LayoutNodeId, LayoutError> {
        let id = self
            .inner
            .new_leaf_with_context(style.to_taffy(), measure)?;
        self.own_nodes.insert(id);
        Ok(LayoutNodeId(id))
    }

    /// 子を持つコンテナノードを追加する。
    pub fn new_with_children(
        &mut self,
        style: LayoutStyle,
        children: &[LayoutNodeId],
    ) -> Result<LayoutNodeId, LayoutError> {
        if !children.iter().all(|c| self.own_nodes.contains(&c.0)) {
            return Err(LayoutError::InvalidNode);
        }
        let child_ids: Vec<taffy::NodeId> = children.iter().map(|c| c.0).collect();
        let id = self.inner.new_with_children(style.to_taffy(), &child_ids)?;
        self.own_nodes.insert(id);
        Ok(LayoutNodeId(id))
    }

    /// `root` を起点にツリー全体のサイズ計測・配置を計算する。
    ///
    /// `available` の各成分が NaN・無限大・負値の場合は計算前に
    /// [`LayoutError::InvalidConstraint`] を返す（外部由来になり得るウィンドウサイズ等の
    /// 数値を panic させない、security.md の DoS 耐性方針）。
    pub fn compute_layout(
        &mut self,
        root: LayoutNodeId,
        available: LayoutSize,
    ) -> Result<(), LayoutError> {
        if !self.own_nodes.contains(&root.0) {
            return Err(LayoutError::InvalidNode);
        }
        validate_available_component(available.width)?;
        validate_available_component(available.height)?;

        let available_space = taffy::geometry::Size {
            width: taffy::style::AvailableSpace::Definite(available.width),
            height: taffy::style::AvailableSpace::Definite(available.height),
        };

        self.inner.compute_layout_with_measure(
            root.0,
            available_space,
            |known_dimensions, available_space, _node_id, node_context, _style| {
                if let taffy::geometry::Size {
                    width: Some(width),
                    height: Some(height),
                } = known_dimensions
                {
                    return taffy::geometry::Size { width, height };
                }
                match node_context {
                    None => taffy::geometry::Size::ZERO,
                    Some(measure) => {
                        // 片軸のみ既知（例: 幅固定・高さ auto）の場合も、既知側は
                        // `AvailableSpace::Definite` として `Measure` へ伝播させる。
                        // ここで `available_space` をそのまま使うと、既知の幅制約が
                        // 無視され、テキスト折り返し高さのようなコンテンツ依存サイズが
                        // 誤った制約（親の available_space）で計算されてしまう
                        // （Bugbot 指摘: PR #356 review comment 3683040107）。
                        let width = known_dimensions
                            .width
                            .map(AvailableSpace::Definite)
                            .unwrap_or_else(|| to_fandhe_available_space(available_space.width));
                        let height = known_dimensions
                            .height
                            .map(AvailableSpace::Definite)
                            .unwrap_or_else(|| to_fandhe_available_space(available_space.height));
                        let constraints = LayoutConstraints { width, height };
                        let size = measure.measure(constraints);
                        taffy::geometry::Size {
                            width: known_dimensions.width.unwrap_or(size.width),
                            height: known_dimensions.height.unwrap_or(size.height),
                        }
                    }
                }
            },
        )?;
        Ok(())
    }

    /// `compute_layout` 後の `node` の配置結果（親相対の原点 + サイズ）を読む。
    pub fn layout(&self, node: LayoutNodeId) -> Result<LayoutRect, LayoutError> {
        if !self.own_nodes.contains(&node.0) {
            return Err(LayoutError::InvalidNode);
        }
        let layout = self.inner.layout(node.0)?;
        Ok(LayoutRect::new(
            LayoutPoint::new(layout.location.x, layout.location.y),
            LayoutSize::new(layout.size.width, layout.size.height),
        ))
    }
}

impl Default for LayoutTree {
    fn default() -> Self {
        Self::new()
    }
}

/// taffy の `AvailableSpace` を `ui::layout` 公開の [`AvailableSpace`] へ写像する。
fn to_fandhe_available_space(value: taffy::style::AvailableSpace) -> AvailableSpace {
    match value {
        taffy::style::AvailableSpace::Definite(v) => AvailableSpace::Definite(v),
        taffy::style::AvailableSpace::MinContent => AvailableSpace::MinContent,
        taffy::style::AvailableSpace::MaxContent => AvailableSpace::MaxContent,
    }
}

/// `compute_layout` の入口で利用可能サイズ 1 軸分を検証する。
///
/// NaN・無限大・負値は taffy 内部でも未定義動作の温床になり得るため、panic を
/// API 契約にしない方針（coding-rust.md）に従いここで弾く。
fn validate_available_component(value: f32) -> Result<(), LayoutError> {
    if !value.is_finite() || value < 0.0 {
        Err(LayoutError::InvalidConstraint)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::layout::style::{Dimension, FlexDirection};

    #[derive(Debug)]
    struct FixedTextMeasure {
        char_width: f32,
        char_count: usize,
    }

    impl Measure for FixedTextMeasure {
        fn measure(&self, constraints: LayoutConstraints) -> LayoutSize {
            let natural_width = self.char_width * self.char_count as f32;
            let width = match constraints.width {
                AvailableSpace::Definite(available) => natural_width.min(available),
                AvailableSpace::MinContent => 0.0,
                AvailableSpace::MaxContent => natural_width,
            };
            let lines = if width <= 0.0 {
                self.char_count as f32
            } else {
                (natural_width / width).ceil().max(1.0)
            };
            LayoutSize::new(width, lines * 10.0)
        }
    }

    /// スペーシングトークン（`spacing.md` = 16px）を [`LayoutStyle::with_gap_token`]
    /// 経由で `gap` に配線し、`compute_layout` の子オフセットへ実際に反映される
    /// ことを検証する（TASK-8.4-S15、#171。`vertical_stack_accumulates_y_with_gap`
    /// の生 `f32` 版と対になるトークン経由のエンドツーエンド検証）。
    #[test]
    fn vertical_stack_gap_token_matches_resolved_spacing_value() {
        use crate::token::{
            TokenPath, TokenRef, TokenResolver, TokenSet, TokenValue, spacing_token_set,
        };

        let base = spacing_token_set().expect("スペーシングトークン構築は成功する");
        let resolver = TokenResolver::new(base, TokenSet::new());
        let path = TokenPath::from_dotted("spacing.md").expect("パスは有効");
        let reference = TokenRef::new(path);
        let resolved = resolver
            .resolve(&reference)
            .expect("spacing.md は既定トークンセットに存在する");
        let spacing = match resolved.value() {
            TokenValue::Spacing(value) => *value,
            other => panic!("spacing.md は Spacing 値であるべきです: {other:?}"),
        };

        let mut tree = LayoutTree::new();
        let leaf_style = LayoutStyle {
            width: Dimension::Points(50.0),
            height: Dimension::Points(20.0),
            ..Default::default()
        };
        let children: Vec<_> = (0..2).map(|_| tree.new_leaf(leaf_style).unwrap()).collect();
        let root_style = LayoutStyle {
            direction: FlexDirection::Column,
            width: Dimension::Points(50.0),
            height: Dimension::Auto,
            ..Default::default()
        }
        .with_gap_token(spacing);
        let root = tree.new_with_children(root_style, &children).unwrap();
        tree.compute_layout(root, LayoutSize::new(50.0, 200.0))
            .unwrap();

        let l0 = tree.layout(children[0]).unwrap();
        let l1 = tree.layout(children[1]).unwrap();
        assert_eq!(l0.origin.y, 0.0);
        // 2 番目の子の origin.y = 1 番目の高さ(20) + gap(spacing.md = 16) = 36。
        assert_eq!(l1.origin.y, 36.0);
    }

    #[test]
    fn vertical_stack_accumulates_y_with_gap() {
        let mut tree = LayoutTree::new();
        let style = LayoutStyle {
            width: Dimension::Points(50.0),
            height: Dimension::Points(20.0),
            ..Default::default()
        };
        let children: Vec<_> = (0..3).map(|_| tree.new_leaf(style).unwrap()).collect();
        let root_style = LayoutStyle {
            direction: FlexDirection::Column,
            gap: 5.0,
            width: Dimension::Points(50.0),
            height: Dimension::Auto,
            ..Default::default()
        };
        let root = tree.new_with_children(root_style, &children).unwrap();
        tree.compute_layout(root, LayoutSize::new(50.0, 200.0))
            .unwrap();

        let l0 = tree.layout(children[0]).unwrap();
        let l1 = tree.layout(children[1]).unwrap();
        let l2 = tree.layout(children[2]).unwrap();
        assert_eq!(l0.origin.y, 0.0);
        assert_eq!(l1.origin.y, 25.0);
        assert_eq!(l2.origin.y, 50.0);
    }

    #[test]
    fn horizontal_row_accumulates_x() {
        let mut tree = LayoutTree::new();
        let style = LayoutStyle {
            width: Dimension::Points(30.0),
            height: Dimension::Points(20.0),
            ..Default::default()
        };
        let children: Vec<_> = (0..2).map(|_| tree.new_leaf(style).unwrap()).collect();
        let root_style = LayoutStyle {
            direction: FlexDirection::Row,
            width: Dimension::Points(100.0),
            ..Default::default()
        };
        let root = tree.new_with_children(root_style, &children).unwrap();
        tree.compute_layout(root, LayoutSize::new(100.0, 100.0))
            .unwrap();

        let l0 = tree.layout(children[0]).unwrap();
        let l1 = tree.layout(children[1]).unwrap();
        assert_eq!(l0.origin.x, 0.0);
        assert_eq!(l1.origin.x, 30.0);
    }

    #[test]
    fn measure_leaf_respects_definite_and_max_content_constraints() {
        let mut tree = LayoutTree::new();
        let measured = tree
            .new_leaf_with_measure(
                LayoutStyle::default(),
                Box::new(FixedTextMeasure {
                    char_width: 10.0,
                    char_count: 20,
                }),
            )
            .unwrap();
        let root_style = LayoutStyle {
            width: Dimension::Points(100.0),
            height: Dimension::Auto,
            ..Default::default()
        };
        let root = tree.new_with_children(root_style, &[measured]).unwrap();
        tree.compute_layout(root, LayoutSize::new(100.0, 500.0))
            .unwrap();
        let definite_layout = tree.layout(measured).unwrap();
        assert!(definite_layout.size.width <= 100.0);
        assert!(
            definite_layout.size.height > 10.0,
            "折返しにより複数行分の高さになるはず"
        );

        let mut tree2 = LayoutTree::new();
        let measured2 = tree2
            .new_leaf_with_measure(
                LayoutStyle::default(),
                Box::new(FixedTextMeasure {
                    char_width: 10.0,
                    char_count: 20,
                }),
            )
            .unwrap();
        tree2
            .compute_layout(measured2, LayoutSize::new(1000.0, 1000.0))
            .unwrap();
        let max_content_layout = tree2.layout(measured2).unwrap();
        assert_eq!(max_content_layout.size.width, 200.0);
        assert_eq!(max_content_layout.size.height, 10.0);
    }

    #[test]
    fn nested_container_with_padding_is_parent_relative() {
        let mut tree = LayoutTree::new();
        let leaf_style = LayoutStyle {
            width: Dimension::Points(10.0),
            height: Dimension::Points(10.0),
            ..Default::default()
        };
        let leaf = tree.new_leaf(leaf_style).unwrap();
        let inner_style = LayoutStyle {
            padding: 5.0,
            width: Dimension::Points(40.0),
            height: Dimension::Points(40.0),
            ..Default::default()
        };
        let inner = tree.new_with_children(inner_style, &[leaf]).unwrap();
        let outer_style = LayoutStyle {
            width: Dimension::Points(100.0),
            height: Dimension::Points(100.0),
            ..Default::default()
        };
        let outer = tree.new_with_children(outer_style, &[inner]).unwrap();
        tree.compute_layout(outer, LayoutSize::new(100.0, 100.0))
            .unwrap();

        let leaf_layout = tree.layout(leaf).unwrap();
        assert_eq!(
            leaf_layout.origin,
            LayoutPoint::new(5.0, 5.0),
            "親（inner）相対でパディング分オフセットされる"
        );
    }

    #[test]
    fn flex_grow_distributes_remaining_space() {
        let mut tree = LayoutTree::new();
        let grow_style = LayoutStyle {
            width: Dimension::Auto,
            height: Dimension::Points(20.0),
            flex_grow: 1.0,
            ..Default::default()
        };
        let fixed_style = LayoutStyle {
            width: Dimension::Points(20.0),
            height: Dimension::Points(20.0),
            ..Default::default()
        };
        let grow_child = tree.new_leaf(grow_style).unwrap();
        let fixed_child = tree.new_leaf(fixed_style).unwrap();
        let root_style = LayoutStyle {
            direction: FlexDirection::Row,
            width: Dimension::Points(100.0),
            ..Default::default()
        };
        let root = tree
            .new_with_children(root_style, &[grow_child, fixed_child])
            .unwrap();
        tree.compute_layout(root, LayoutSize::new(100.0, 100.0))
            .unwrap();

        let grow_layout = tree.layout(grow_child).unwrap();
        assert_eq!(grow_layout.size.width, 80.0);
    }

    #[test]
    fn invalid_node_id_from_other_tree_is_rejected() {
        let mut tree_a = LayoutTree::new();
        let node_in_a = tree_a.new_leaf(LayoutStyle::default()).unwrap();

        let tree_b = LayoutTree::new();
        let result = tree_b.layout(node_in_a);
        assert_eq!(result, Err(LayoutError::InvalidNode));
    }

    #[test]
    fn measure_receives_known_width_when_height_is_auto() {
        // Bugbot 指摘（PR #356 review comment 3683040107）の回帰テスト:
        // 幅固定・高さ auto のように片軸のみ既知の場合でも、既知の幅制約が
        // `Measure` へ伝播し、`available_space`（ここでは 1000.0）ではなく
        // 既知の幅（50.0）を基準に折り返し高さが計算されることを検証する。
        let mut tree = LayoutTree::new();
        let leaf_style = LayoutStyle {
            width: Dimension::Points(50.0),
            height: Dimension::Auto,
            ..Default::default()
        };
        let leaf = tree
            .new_leaf_with_measure(
                leaf_style,
                Box::new(FixedTextMeasure {
                    char_width: 10.0,
                    char_count: 20,
                }),
            )
            .unwrap();
        tree.compute_layout(leaf, LayoutSize::new(1000.0, 1000.0))
            .unwrap();
        let layout = tree.layout(leaf).unwrap();
        assert_eq!(
            layout.size.width, 50.0,
            "既知の幅制約（スタイルの固定幅）が優先されるはず"
        );
        assert_eq!(
            layout.size.height, 40.0,
            "幅 50 制約下で 20 文字（char_width 10）は 4 行分の高さ（40.0）になるはず\
             （available_space の 1000.0 を使うと 1 行・10.0 になってしまう）"
        );
    }

    #[test]
    fn nan_available_size_is_rejected_without_panic() {
        let mut tree = LayoutTree::new();
        let root = tree.new_leaf(LayoutStyle::default()).unwrap();
        let result = tree.compute_layout(root, LayoutSize::new(f32::NAN, 100.0));
        assert_eq!(result, Err(LayoutError::InvalidConstraint));
    }

    #[test]
    fn negative_available_size_is_rejected_without_panic() {
        let mut tree = LayoutTree::new();
        let root = tree.new_leaf(LayoutStyle::default()).unwrap();
        let result = tree.compute_layout(root, LayoutSize::new(100.0, -1.0));
        assert_eq!(result, Err(LayoutError::InvalidConstraint));
    }
}
