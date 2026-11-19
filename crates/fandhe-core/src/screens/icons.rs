//! `icon` / `icon_bound` の確認用画面（Issue #1437）。
//!
//! `FANDHE_PROXY_APP=icons` で選ばれ、`crates/fandhe-gallery/src/screens/registry.rs`
//! から呼ばれる（`screens/mod.rs` の `pub mod icons;` と対）。語彙 [`IconName`] の全名前を格子で並べ、
//! 状態束縛のアイコンをボタンで次の名前へ進めて追従を見る。ラベルは読み上げ名で、画面には出ない。

use crate::{Grid, Icon, IconName, Screen, WidgetId};

/// 状態束縛のアイコンが指す名前の位置（`IconName::ALL` の添字）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IconsState {
    /// `IconName::ALL` の添字。
    pub index: usize,
}

fn current(state: &IconsState) -> IconName {
    IconName::ALL[state.index % IconName::ALL.len()]
}

/// ボタンを押すたびに次の名前へ進める。
fn next(state: &mut IconsState) {
    state.index = (state.index + 1) % IconName::ALL.len();
}

/// 状態束縛の 1 つ・進めるボタン・大きさの違い・語彙の全名前の格子を縦に並べた画面を組み立てる。
pub fn icons_screen() -> Screen<IconsState> {
    let id = WidgetId::from_raw;
    let grid = IconName::ALL.iter().zip(100u64..).fold(
        Grid::new(id(1)).with_columns(8).with_spacing(8.0),
        |grid, (name, n)| grid.with_child(Icon::fixed(id(n), *name, format!("{name:?}"))),
    );
    Screen::new("アイコン")
        .text("状態束縛（ボタンで次の名前へ）")
        .icon_bound(id(4), current, "選択中のアイコン")
        .button(id(5), "次へ", next)
        .text("大きさ（既定 24・48）")
        .icon(id(2), IconName::Star, "既定の大きさの星")
        .with_node(crate::Node::Widget(Box::new(
            Icon::<IconsState>::fixed(id(3), IconName::Star, "大きい星").with_size(48.0),
        )))
        .text("語彙の全名前")
        .grid(grid)
}
