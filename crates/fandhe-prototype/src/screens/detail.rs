//! 詳細画面（`Route::Detail`）。
//!
//! `docs/architecture/prototype-app-design.md` 2 章の構成（選択中項目のタイトル
//! `Text`・メモ編集 `TextField`・「一覧へ戻る」`Button`）を実装する（S3 #50）。
//! [`crate::nav::current_screen`] から `Route::Detail` のディスパッチ先として呼ばれる。
//!
//! 「一覧へ戻る」ボタンは [`crate::nav::go_to_list`]（S5 #52 が実装本体を持つ）を
//! そのまま使う。本ファイルに暫定ローカル実装は置かない。

use fandhe_core::Screen;
use fandhe_core::ui::{Node, WidgetId};

use crate::nav::go_to_list;
use crate::state::{AppState, get_memo, selected_item, set_memo};

/// 選択中項目のタイトルを `Text` として展開する（`ReadFn<AppState, Vec<Node<AppState>>>`
/// 互換の関数ポインタ、`.list()` ビルダー経由で状態依存テキストを表現する）。
///
/// `fandhe-core` の `Screen::text()` は静的文字列専用のため、`AppState::selected` に
/// 追従するタイトル表示には `Node::List` の子要素展開を用いる（設計判断は親 issue #47
/// / TASK-2.2 の計画を参照）。選択対象が存在しない（`items` が空）場合は panic せず
/// フォールバック文言を返す（panic を API 契約にしない、coding-rust.md）。
fn expand_selected_title(state: &AppState) -> Vec<Node<AppState>> {
    match selected_item(state) {
        Some(item) => vec![Node::Text(item.title.clone())],
        None => vec![Node::Text("（項目がありません）".to_string())],
    }
}

/// 詳細画面を構築する。タイトルは [`crate::nav::current_screen`] のテストで
/// 他画面と判別するためのキーとしても使うため変更しない。
pub fn detail_screen() -> Screen<AppState> {
    Screen::new("詳細")
        .list(expand_selected_title)
        .text_field("メモ", get_memo, set_memo)
        .button(WidgetId::from_raw(1), "一覧へ戻る", go_to_list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Item, Route};
    use fandhe_core::click_button;

    fn state_with_items() -> AppState {
        AppState {
            items: vec![
                Item {
                    title: "1件目".to_string(),
                    memo: "メモ1".to_string(),
                },
                Item {
                    title: "2件目".to_string(),
                    memo: "メモ2".to_string(),
                },
            ],
            selected: 1,
            ..Default::default()
        }
    }

    /// ノード構成・挿入順（`List` → `TextField` → `Button`）とラベルを検証する。
    #[test]
    fn detail_screen_has_list_text_field_and_button_in_order() {
        let screen = detail_screen();
        assert_eq!(screen.title(), "詳細");
        assert_eq!(screen.len(), 3);

        let labels: Vec<&str> = screen
            .iter()
            .map(|node| match node {
                Node::List { .. } => "list",
                Node::VirtualList { .. } => "virtual_list",
                Node::TextField { label, .. } => label.as_str(),
                Node::Button { label, .. } => label.as_str(),
                // 本プロトタイプ画面は Issue #491 時点で `IndexedButton` を使わない
                // （動的生成子の対話ノードは未追加）。到達しないはずの分岐だが、
                // 網羅性のために明示する。
                Node::IndexedButton { label, .. } => label.as_str(),
                Node::Text(_) => "text",
                // 本プロトタイプ画面は Issue #581 時点で `DynamicText` を使わない
                // （動的テキストは未導入）。到達しないはずの分岐だが、網羅性のために明示する。
                Node::DynamicText { .. } => "dynamic_text",
                // 本プロトタイプ画面は Issue #812 時点で `StyledText`（`.error_text()`）を
                // 使わない。到達しないはずの分岐だが、網羅性のために明示する。
                Node::StyledText { .. } => "styled_text",
                // 本プロトタイプ画面は TASK-8.3-S2 時点で `Widget` を使わない
                // （個別ウィジェット実装は S7 以降のスコープ）。到達しないはずの分岐だが、
                // 網羅性のために明示する（Issue #97）。
                Node::Widget(_) => "widget",
            })
            .collect();
        assert_eq!(labels, vec!["list", "メモ", "一覧へ戻る"]);
    }

    /// `detail_screen_builds_with_at_least_one_node`（旧スタブテスト）相当の
    /// 非空検証を維持する。
    #[test]
    fn detail_screen_builds_with_at_least_one_node() {
        let screen = detail_screen();
        assert!(!screen.is_empty());
    }

    /// 選択中項目が範囲内であれば、そのタイトルが `Text` として展開されることを検証する。
    #[test]
    fn expand_selected_title_shows_selected_item_title_when_in_range() {
        let state = state_with_items();
        let nodes = expand_selected_title(&state);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            Node::Text(text) => assert_eq!(text, "2件目"),
            _ => panic!("Text バリアントであるべき"),
        }
    }

    /// `selected` が範囲外でも `selected_item` の境界防御により panic せず、
    /// 末尾要素のタイトルへ丸められて展開されることを検証する。
    #[test]
    fn expand_selected_title_saturates_when_selected_out_of_range() {
        let mut state = state_with_items();
        state.selected = 99;
        let nodes = expand_selected_title(&state);
        match &nodes[0] {
            Node::Text(text) => assert_eq!(text, "2件目"),
            _ => panic!("Text バリアントであるべき"),
        }
    }

    /// `items` が空でも panic せず、フォールバック文言を返すことを検証する。
    #[test]
    fn expand_selected_title_falls_back_when_items_is_empty() {
        let state = AppState::default();
        let nodes = expand_selected_title(&state);
        match &nodes[0] {
            Node::Text(text) => assert_eq!(text, "（項目がありません）"),
            _ => panic!("Text バリアントであるべき"),
        }
    }

    /// 画面から取り出した `TextField` の `get`/`set` 関数ポインタ経由で
    /// メモの読み書きが `AppState` を往復することを検証する。
    #[test]
    fn text_field_get_set_round_trips_memo_via_app_state() {
        let screen = detail_screen();
        let (get, set) = screen
            .iter()
            .find_map(|node| match node {
                Node::TextField { get, set, .. } => Some((*get, *set)),
                _ => None,
            })
            .expect("TextField ノードが存在するべき");

        let mut state = state_with_items();
        assert_eq!(get(&state), "メモ2");

        set(&mut state, "更新後のメモ".to_string());
        assert_eq!(get(&state), "更新後のメモ");
        assert_eq!(state.items[1].memo, "更新後のメモ");
    }

    /// 「一覧へ戻る」ボタン押下で `route` が `Detail` から `List` へ遷移することを検証する。
    #[test]
    fn clicking_back_to_list_button_changes_route_to_list() {
        let screen = detail_screen();
        let mut state = AppState {
            route: Route::Detail,
            ..state_with_items()
        };

        assert!(click_button(&screen, "一覧へ戻る", &mut state));
        assert_eq!(state.route, Route::List);
    }
}
