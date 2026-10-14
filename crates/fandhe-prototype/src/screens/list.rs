//! 一覧画面（`Route::List`）。
//!
//! `docs/architecture/prototype-app-design.md` 2 章のノード構成（見出し `Text`・
//! 項目一覧 `List`・「項目を追加」「次へ/前へ」「詳細を開く」「設定」の各
//! `Button`）に従う（S2 #49）。
//!
//! # 画面遷移ハンドラの扱い（S5 #52 で `nav.rs` へ集約済み）
//!
//! 画面遷移ハンドラ（`go_to_detail` / `go_to_settings` / `select_next` /
//! `select_prev`）は [`crate::nav`]（S5 #52 が実装本体を持つ）の関数ポインタを
//! そのまま `Screen::button` へ渡す。本ファイルに暫定ローカル実装は置かない。
//! 「項目を追加」は一覧画面固有の操作のため本モジュールで確定実装する。

use fandhe_core::{Node, Screen, WidgetId};

use crate::nav::{go_to_detail, go_to_settings, select_next, select_prev};
use crate::state::{AppState, Item};

/// `state.items` の 1 件を一覧行として表す `Node`（タイトルのみの `Text`）に変換する。
///
/// `Node::List` の `expand` は `fn(&S) -> Vec<Node<S>>` の関数ポインタに固定されるため
/// （`fandhe_core` の `Node::List` 参照）、本関数自身も関数ポインタとして
/// `Screen::list` へそのまま渡せる。
fn item_rows(state: &AppState) -> Vec<Node<AppState>> {
    state
        .items
        .iter()
        .map(|item: &Item| Node::Text(item.title.clone()))
        .collect()
}

/// 「項目を追加」ボタンのハンドラ。新規項目を末尾へ追加し、選択をその項目へ移す。
///
/// タイトルは一意性を要さない簡易プロトタイプのため、追加順の連番
/// （「新規項目 N」）とする（詳細な命名 UI は本 issue のスコープ外）。
fn add_item(state: &mut AppState) {
    let n = state.items.len() + 1;
    state.items.push(Item {
        title: format!("新規項目 {n}"),
        memo: String::new(),
    });
    state.selected = state.items.len() - 1;
}

/// 一覧画面を構築する。
///
/// [`crate::nav::current_screen`] が `Route::List` のとき本関数を呼ぶ
/// （ディスパッチ本体は S1 #48）。項目選択は「次へ/前へ」ボタンによる添字移動方式
/// とする（`List` 展開内ノードの個別クリック検知ができない DSL 制約に由来する設計判断、
/// `docs/architecture/prototype-app-design.md` 5 章）。
pub fn list_screen() -> Screen<AppState> {
    Screen::new("一覧")
        .text("タスクメモ一覧")
        .list(item_rows)
        .button(WidgetId::from_raw(1), "項目を追加", add_item)
        .button(WidgetId::from_raw(2), "次へ", select_next)
        .button(WidgetId::from_raw(3), "前へ", select_prev)
        .button(WidgetId::from_raw(4), "詳細を開く", go_to_detail)
        .button(WidgetId::from_raw(5), "設定", go_to_settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, Route};
    use fandhe_core::click_button;

    #[test]
    fn list_screen_builds_with_expected_title() {
        let screen = list_screen();
        assert_eq!(screen.title(), "一覧");
        assert!(!screen.is_empty());
    }

    /// `Screen::visit_expanded`（DSL の公開走査 API）経由で `List` ノードが
    /// `state.items` から実際に展開されることを確認する（プライベート関数
    /// `item_rows` の単体呼び出しだけでは、`.list(item_rows)` が `list_screen()`
    /// の構築結果に組み込まれていることまでは検証できないため）。
    #[test]
    fn list_screen_expands_items_via_public_traversal() {
        let screen = list_screen();
        let state = AppState {
            items: vec![
                Item {
                    title: "1件目".to_string(),
                    memo: String::new(),
                },
                Item {
                    title: "2件目".to_string(),
                    memo: String::new(),
                },
            ],
            ..Default::default()
        };

        let mut texts = Vec::new();
        screen.visit_expanded(&state, |node| {
            if let Node::Text(text) = node {
                texts.push(text.clone());
            }
        });

        assert!(texts.contains(&"1件目".to_string()));
        assert!(texts.contains(&"2件目".to_string()));
    }

    /// `click_button`（DSL の公開ボタン発火 API）経由で「項目を追加」ボタンが
    /// 実際に発火し、`.list()` の後段に置かれたボタン群も
    /// `Screen::iter`（トップレベル走査）から見つかることを確認する
    /// （design doc 5 章の「List 展開内は線形走査対象外」という制約が、
    /// トップレベルの Button 自体には影響しないことの回帰検知）。
    #[test]
    fn click_add_item_button_via_public_dispatch() {
        let screen = list_screen();
        let mut state = AppState::default();

        assert!(click_button(&screen, "項目を追加", &mut state));
        assert_eq!(state.items.len(), 1);
    }

    /// 「詳細を開く」ボタンの発火で `route` が `Route::Detail` に変わることを
    /// 公開 API 経由で確認する。
    #[test]
    fn click_open_detail_button_via_public_dispatch() {
        let screen = list_screen();
        let mut state = AppState::default();

        assert!(click_button(&screen, "詳細を開く", &mut state));
        assert_eq!(state.route, Route::Detail);
    }

    /// 「設定」ボタンの発火で `route` が `Route::Settings` に変わることを
    /// 公開 API 経由で確認する。
    #[test]
    fn click_open_settings_button_via_public_dispatch() {
        let screen = list_screen();
        let mut state = AppState::default();

        assert!(click_button(&screen, "設定", &mut state));
        assert_eq!(state.route, Route::Settings);
    }

    #[test]
    fn item_rows_reflects_items_titles() {
        let state = AppState {
            items: vec![
                Item {
                    title: "1件目".to_string(),
                    memo: String::new(),
                },
                Item {
                    title: "2件目".to_string(),
                    memo: String::new(),
                },
            ],
            ..Default::default()
        };

        let rows = item_rows(&state);
        assert_eq!(rows.len(), 2);
        match &rows[0] {
            Node::Text(text) => assert_eq!(text, "1件目"),
            _ => panic!("Text バリアントであるべき"),
        }
    }

    #[test]
    fn item_rows_is_empty_when_no_items() {
        let state = AppState::default();
        assert!(item_rows(&state).is_empty());
    }

    #[test]
    fn add_item_appends_and_selects_new_item() {
        let mut state = AppState::default();
        add_item(&mut state);
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].title, "新規項目 1");
        assert_eq!(state.selected, 0);

        add_item(&mut state);
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[1].title, "新規項目 2");
        assert_eq!(state.selected, 1);
    }

    // select_next / select_prev / go_to_detail / go_to_settings の単体テストは
    // `crate::nav`（S5 #52 が実装本体・テストの正式な置き場所）へ移設済み。
    // ここでは `click_button` 経由の画面レベルテスト（上記）のみを維持する。
}
