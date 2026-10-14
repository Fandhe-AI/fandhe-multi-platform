//! レイアウトプロトコル（サイズ計測・配置）基盤の結合テスト（TASK-8.3-S4 / REQ-8、
//! Issue #99）。
//!
//! `fandhe_core::ui::layout` の公開 API のみを用い、縦・横方向のスタック配置・
//! テキスト風計測（`Measure` 実装）・ネスト・`flex_grow` によるスペース分配・
//! エラー系（未登録ノード参照・不正な制約値）を検証する。ユニットテスト
//! （`src/ui/layout/tree.rs` の `#[cfg(test)] mod tests`）と観点は重なるが、こちらは
//! クレート外部から公開 API のみを使う経路として維持する（`tests/sample_screen.rs` と
//! 同じ方針）。

use fandhe_core::ui::{
    AvailableSpace, Dimension, FlexDirection, LayoutConstraints, LayoutError, LayoutSize,
    LayoutStyle, LayoutTree, Measure,
};

#[derive(Debug)]
struct CharGridMeasure {
    char_width: f32,
    char_count: usize,
}

impl Measure for CharGridMeasure {
    fn measure(&self, constraints: LayoutConstraints) -> LayoutSize {
        let natural_width = self.char_width * self.char_count as f32;
        match constraints.width {
            AvailableSpace::Definite(available) if available < natural_width => {
                let lines = (natural_width / available).ceil().max(1.0);
                LayoutSize::new(available, lines * 10.0)
            }
            _ => LayoutSize::new(natural_width, 10.0),
        }
    }
}

#[test]
fn vertical_stack_places_children_top_to_bottom_with_gap() {
    let mut tree = LayoutTree::new();
    let child_style = LayoutStyle {
        width: Dimension::Points(40.0),
        height: Dimension::Points(10.0),
        ..Default::default()
    };
    let children = [
        tree.new_leaf(child_style).unwrap(),
        tree.new_leaf(child_style).unwrap(),
    ];
    let root_style = LayoutStyle {
        direction: FlexDirection::Column,
        gap: 4.0,
        width: Dimension::Points(40.0),
        ..Default::default()
    };
    let root = tree.new_with_children(root_style, &children).unwrap();
    tree.compute_layout(root, LayoutSize::new(40.0, 100.0))
        .unwrap();

    assert_eq!(tree.layout(children[0]).unwrap().origin.y, 0.0);
    assert_eq!(tree.layout(children[1]).unwrap().origin.y, 14.0);
}

#[test]
fn measure_leaf_wraps_under_definite_width_constraint() {
    let mut tree = LayoutTree::new();
    let measured = tree
        .new_leaf_with_measure(
            LayoutStyle::default(),
            Box::new(CharGridMeasure {
                char_width: 8.0,
                char_count: 30,
            }),
        )
        .unwrap();
    let root_style = LayoutStyle {
        width: Dimension::Points(80.0),
        height: Dimension::Auto,
        ..Default::default()
    };
    let root = tree.new_with_children(root_style, &[measured]).unwrap();
    tree.compute_layout(root, LayoutSize::new(80.0, 400.0))
        .unwrap();

    let layout = tree.layout(measured).unwrap();
    assert!(layout.size.width <= 80.0);
    assert!(
        layout.size.height > 10.0,
        "折返しが起きて 1 行分より高くなるはず"
    );
}

#[test]
fn nested_containers_report_parent_relative_coordinates() {
    let mut tree = LayoutTree::new();
    let leaf = tree
        .new_leaf(LayoutStyle {
            width: Dimension::Points(10.0),
            height: Dimension::Points(10.0),
            ..Default::default()
        })
        .unwrap();
    let inner = tree
        .new_with_children(
            LayoutStyle {
                padding: 8.0,
                width: Dimension::Points(30.0),
                height: Dimension::Points(30.0),
                ..Default::default()
            },
            &[leaf],
        )
        .unwrap();
    let outer = tree
        .new_with_children(
            LayoutStyle {
                width: Dimension::Points(100.0),
                height: Dimension::Points(100.0),
                ..Default::default()
            },
            &[inner],
        )
        .unwrap();
    tree.compute_layout(outer, LayoutSize::new(100.0, 100.0))
        .unwrap();

    let leaf_layout = tree.layout(leaf).unwrap();
    assert_eq!(leaf_layout.origin.x, 8.0);
    assert_eq!(leaf_layout.origin.y, 8.0);
}

#[test]
fn flex_grow_child_fills_remaining_row_space() {
    let mut tree = LayoutTree::new();
    let fixed = tree
        .new_leaf(LayoutStyle {
            width: Dimension::Points(30.0),
            height: Dimension::Points(10.0),
            ..Default::default()
        })
        .unwrap();
    let growing = tree
        .new_leaf(LayoutStyle {
            height: Dimension::Points(10.0),
            flex_grow: 1.0,
            ..Default::default()
        })
        .unwrap();
    let root = tree
        .new_with_children(
            LayoutStyle {
                direction: FlexDirection::Row,
                width: Dimension::Points(100.0),
                ..Default::default()
            },
            &[fixed, growing],
        )
        .unwrap();
    tree.compute_layout(root, LayoutSize::new(100.0, 50.0))
        .unwrap();

    assert_eq!(tree.layout(growing).unwrap().size.width, 70.0);
}

#[test]
fn referencing_node_from_a_different_tree_returns_error_not_panic() {
    let mut tree_a = LayoutTree::new();
    let node_a = tree_a.new_leaf(LayoutStyle::default()).unwrap();

    let tree_b = LayoutTree::new();
    assert_eq!(tree_b.layout(node_a), Err(LayoutError::InvalidNode));
}

#[test]
fn non_finite_or_negative_available_size_is_reported_as_error() {
    let mut tree = LayoutTree::new();
    let root = tree.new_leaf(LayoutStyle::default()).unwrap();

    assert_eq!(
        tree.compute_layout(root, LayoutSize::new(f32::INFINITY, 10.0)),
        Err(LayoutError::InvalidConstraint)
    );
    assert_eq!(
        tree.compute_layout(root, LayoutSize::new(10.0, -5.0)),
        Err(LayoutError::InvalidConstraint)
    );
}
