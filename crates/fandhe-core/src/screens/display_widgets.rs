//! 表示専用部品（`progress_bar` / `spinner` / `separator`）を並べた確認用画面（Issue #1428）。
//!
//! `FANDHE_PROXY_APP=display-widgets` で選ばれ、`crates/fandhe-gallery/src/screens/registry.rs`
//! から呼ばれる（`screens/mod.rs` の `pub mod display_widgets;` と対）。
//! `fandhe-render-egui` が実際に描くことの目視確認用。ボタンで割合を進め、棒が状態に追従することも見る。

use crate::{Screen, Separator, WidgetId};

/// 進捗の表示値を持つ状態。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayWidgetsState {
    /// `progress_bar` が読む割合（0.0..=1.0）。
    pub fraction: f32,
}

impl Default for DisplayWidgetsState {
    fn default() -> Self {
        Self { fraction: 0.4 }
    }
}

fn fraction(state: &DisplayWidgetsState) -> f32 {
    state.fraction
}

/// ボタンを押すたびに割合を 0.1 進め、1.0 を超えたら 0 へ戻す。
fn advance(state: &mut DisplayWidgetsState) {
    state.fraction = if state.fraction >= 0.95 {
        0.0
    } else {
        state.fraction + 0.1
    };
}

/// 棒（ラベル付き・なし）・区切り線・スピナー・割合を進めるボタンを縦に並べた画面を組み立てる。
pub fn display_widgets_screen() -> Screen<DisplayWidgetsState> {
    Screen::new("表示部品")
        .text("進捗バー（ラベル付き）")
        .progress_bar(WidgetId::from_raw(1), "ダウンロード", fraction)
        .text("進捗バー（ラベルなし）")
        .progress_bar(WidgetId::from_raw(2), "", fraction)
        .button(WidgetId::from_raw(3), "進める", advance)
        .separator(Separator::new(WidgetId::from_raw(4)))
        .text("スピナー")
        .spinner(WidgetId::from_raw(5))
}
