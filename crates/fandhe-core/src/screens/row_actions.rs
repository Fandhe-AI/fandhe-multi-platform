//! 一覧の行内に複数の操作を横に並べる例「行の操作」（Issue #1288）。
//! 各行に `Node::Row` を置き、題名（行全体のタップの入口）・文字・評価（`CellButton`）・
//! 削除（`IndexedButton`）を横に並べる。文字や余白のタップも入口を押したことになる。
//!
//! `FANDHE_PROXY_APP=row-actions` で選ばれ、`crates/fandhe-gallery/src/screens/registry.rs` から
//! 呼び出される（`screens/mod.rs` の `pub mod row_actions;` と対）。行の添字は `expand` の中で
//! 値として埋め込むため、ハンドラは関数ポインタのまま（REQ-13）。

use crate::ui::Node;
use crate::{Screen, WidgetId};

/// 行の状態。`names` が一覧、`ratings` が各行の評価（0 は未評価）。`opened` は直近に開いた行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowActionsState {
    /// 行の題名。
    pub names: Vec<String>,
    /// 各行の評価（`names` と同じ並び。0 は未評価）。
    pub ratings: Vec<usize>,
    /// 直近に開いた行（行全体のタップか題名ボタン）。
    pub opened: Option<usize>,
}

impl Default for RowActionsState {
    fn default() -> Self {
        RowActionsState {
            names: ["朝のメモ", "買い物", "読書"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ratings: vec![0; 3],
            opened: None,
        }
    }
}

/// 題名ボタン・行全体のタップ: その行を開いたことにする。
pub fn open_row(state: &mut RowActionsState, row: usize) {
    if row < state.names.len() {
        state.opened = Some(row);
    }
}

/// 評価ボタン: `(行, 星の数)` を受けて評価を書く。範囲外は無視する。
pub fn rate_row(state: &mut RowActionsState, (row, stars): (usize, usize)) {
    if let Some(r) = state.ratings.get_mut(row) {
        *r = stars;
    }
}

/// 削除ボタン: 行を取り除き、開いていた行の添字を詰める。範囲外は無視する。
pub fn delete_row(state: &mut RowActionsState, row: usize) {
    if row >= state.names.len() {
        return;
    }
    state.names.remove(row);
    state.ratings.remove(row);
    state.opened = match state.opened {
        Some(o) if o == row => None,
        Some(o) if o > row => Some(o - 1),
        other => other,
    };
}

/// 行ごとの [`WidgetId`] の幅（1 行に 4 つまで）。
const ROW_ID_STRIDE: u64 = 10;

fn rows(state: &RowActionsState) -> Vec<Node<RowActionsState>> {
    state
        .names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let base = ROW_ID_STRIDE * (i as u64 + 1);
            let stars = state.ratings.get(i).copied().unwrap_or(0);
            Node::Row {
                children: vec![
                    Node::IndexedButton {
                        label: name.clone(),
                        index: i,
                        on_click: open_row,
                        id: WidgetId::from_raw(base),
                    },
                    Node::Text(format!("{}件目", i + 1)),
                    Node::CellButton {
                        label: format!("★{}", (stars + 1).min(5)),
                        row: i,
                        col: (stars + 1).min(5),
                        on_click: rate_row,
                        id: WidgetId::from_raw(base + 1),
                    },
                    Node::IndexedButton {
                        label: "削除".to_string(),
                        index: i,
                        on_click: delete_row,
                        id: WidgetId::from_raw(base + 2),
                    },
                ],
                tap: Some(0),
            }
        })
        .collect()
}

fn summary(state: &RowActionsState) -> String {
    match state.opened.and_then(|o| state.names.get(o)) {
        Some(name) => format!("開いた行: {name}"),
        None => "開いた行: なし".to_string(),
    }
}

/// 行の操作画面を組み立てる。
pub fn row_actions_screen() -> Screen<RowActionsState> {
    Screen::new("行の操作").list(rows).dynamic_text(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{click_button_at, render_headless};

    #[test]
    fn rows_render_in_order_with_actions() {
        let screen = row_actions_screen();
        let state = RowActionsState::default();
        let lines = render_headless(&screen, &state);
        assert_eq!(
            &lines[..5],
            ["# 行の操作", "(朝のメモ)", "1件目", "(★1)", "(削除)"].map(String::from)
        );
    }

    #[test]
    fn actions_target_their_own_row() {
        let screen = row_actions_screen();
        let mut state = RowActionsState::default();
        // path = [list, 行, 子]
        assert!(click_button_at(&screen, &[0, 1, 2], "★1", &mut state));
        assert_eq!(state.ratings, vec![0, 1, 0]);
        assert!(click_button_at(&screen, &[0, 2, 0], "読書", &mut state));
        assert_eq!(state.opened, Some(2));
        assert!(click_button_at(&screen, &[0, 1, 3], "削除", &mut state));
        assert_eq!(
            state.names,
            vec!["朝のメモ".to_string(), "読書".to_string()]
        );
        assert_eq!(state.opened, Some(1));
    }
}
