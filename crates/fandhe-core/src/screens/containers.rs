//! `Stack`・`Grid`・`Card`・`Tooltip` の子を並べた確認用画面（Issue #1430）。
//!
//! `FANDHE_PROXY_APP=containers` で選ばれ、`crates/fandhe-gallery/src/screens/registry.rs`
//! から呼ばれる（`screens/mod.rs` の `pub mod containers;` と対）。`fandhe-render-egui` が
//! 子を再帰的に描き、子の操作を状態へ書き戻すことの目視確認用。子は `Checkbox` と `ProgressBar`
//! （`Widget` 側で描かれる部品）を使う。子 `Label` は #1427 の結論待ちで描かれないため使わない。

use crate::ui::FlexDirection;
use crate::{Card, Checkbox, Grid, Node, ProgressBar, Screen, Stack, Tooltip, WidgetId};

/// 子のチェックボックスの値。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContainersState {
    /// `Stack`（縦）の 1 つ目。
    pub stack_a: bool,
    /// `Stack`（横）の 1 つ目。
    pub row_a: bool,
    /// `Stack`（横）の 2 つ目。
    pub row_b: bool,
    /// `Grid` の 1 つ目。
    pub grid_a: bool,
    /// `Grid` の 2 つ目。
    pub grid_b: bool,
    /// `Grid` の 3 つ目（2 行目の左）。
    pub grid_c: bool,
    /// `Card` の中。
    pub card: bool,
    /// `Tooltip` のアンカー。
    pub anchor: bool,
}

fn get_stack_a(s: &ContainersState) -> bool {
    s.stack_a
}
fn set_stack_a(s: &mut ContainersState, v: bool) {
    s.stack_a = v;
}
fn get_row_a(s: &ContainersState) -> bool {
    s.row_a
}
fn set_row_a(s: &mut ContainersState, v: bool) {
    s.row_a = v;
}
fn get_row_b(s: &ContainersState) -> bool {
    s.row_b
}
fn set_row_b(s: &mut ContainersState, v: bool) {
    s.row_b = v;
}
fn get_grid_a(s: &ContainersState) -> bool {
    s.grid_a
}
fn set_grid_a(s: &mut ContainersState, v: bool) {
    s.grid_a = v;
}
fn get_grid_b(s: &ContainersState) -> bool {
    s.grid_b
}
fn set_grid_b(s: &mut ContainersState, v: bool) {
    s.grid_b = v;
}
fn get_grid_c(s: &ContainersState) -> bool {
    s.grid_c
}
fn set_grid_c(s: &mut ContainersState, v: bool) {
    s.grid_c = v;
}
fn get_card(s: &ContainersState) -> bool {
    s.card
}
fn set_card(s: &mut ContainersState, v: bool) {
    s.card = v;
}
fn get_anchor(s: &ContainersState) -> bool {
    s.anchor
}
fn set_anchor(s: &mut ContainersState, v: bool) {
    s.anchor = v;
}
fn progress(_: &ContainersState) -> f32 {
    0.6
}

fn summary(s: &ContainersState) -> String {
    format!(
        "状態: 縦 {} / 横 {}・{} / 格子 {}・{}・{} / カード {} / ヒント {}",
        s.stack_a, s.row_a, s.row_b, s.grid_a, s.grid_b, s.grid_c, s.card, s.anchor
    )
}

fn check(
    id: u64,
    label: &str,
    get: crate::ReadFn<ContainersState, bool>,
    set: crate::WriteFn<ContainersState, bool>,
) -> Checkbox<ContainersState> {
    Checkbox::bound(WidgetId::from_raw(id), label, get, set)
}

/// 縦 `Stack`・横 `Stack`・2 列の `Grid`・`Card`・`Tooltip` を並べた画面を組み立てる。
pub fn containers_screen() -> Screen<ContainersState> {
    let id = WidgetId::from_raw;
    Screen::new("コンテナ")
        .stack(
            Stack::new(id(1))
                .with_spacing(4.0)
                .with_child(check(11, "縦 1", get_stack_a, set_stack_a))
                .with_child(ProgressBar::bound(id(12), "進捗", progress)),
        )
        .stack(
            Stack::new(id(2))
                .with_direction(FlexDirection::Row)
                .with_spacing(24.0)
                .with_child(check(21, "横 1", get_row_a, set_row_a))
                .with_child(check(22, "横 2", get_row_b, set_row_b)),
        )
        .grid(
            Grid::new(id(3))
                .with_columns(2)
                .with_spacing(4.0)
                .with_child(check(31, "格子 1", get_grid_a, set_grid_a))
                .with_child(check(32, "格子 2", get_grid_b, set_grid_b))
                .with_child(check(33, "格子 3", get_grid_c, set_grid_c)),
        )
        .card(
            Card::new(id(4))
                .with_padding(8.0)
                .with_child(check(41, "カードの中", get_card, set_card))
                .with_child(ProgressBar::bound(id(42), "カードの進捗", progress)),
        )
        .with_node(Node::Widget(Box::new(
            Tooltip::fixed(id(5), "ポインタを重ねると出る補足").with_anchor(check(
                51,
                "ヒント付き",
                get_anchor,
                set_anchor,
            )),
        )))
        .dynamic_text(summary)
}
