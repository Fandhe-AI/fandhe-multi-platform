//! 設定画面（`Route::Settings`）。
//!
//! S4 #51（設計書 `docs/architecture/prototype-app-design.md` 2 章の
//! 確定構成: 見出し `Text`・ユーザー名 `TextField`・「一覧へ戻る」`Button`）。
//!
//! 「一覧へ戻る」ボタンは [`crate::nav::go_to_list`]（S5 #52 が実装本体を持つ）に
//! 集約済み（S4 マージ後の統合、本 issue #52 のスコープ）。本ファイルに暫定
//! ローカル実装は置かない。
//!
//! TASK-4.4-S3（REQ-4 / Issue #81）では、`TextField` フォームを含み
//! `<label for>`/`<input id>` 関連付けの効果が最も現れる本画面を
//! `RenderMode::Dom` 指定の例とした（一覧・詳細は既定の `RenderMode::Canvas` のまま）。
//! S4（#82）の a11y/SEO 確認テストの検証対象となる想定。

use fandhe_core::{RenderMode, Screen, WidgetId};

use crate::nav::go_to_list;
use crate::state::{AppState, get_user_name, set_user_name};

/// 設定画面を構築する（設計書 2 章の確定構成）。
///
/// タイトル `"設定"` は [`crate::nav::current_screen`] のディスパッチテストが
/// 他画面と判別するためのキーとして使うため変更しない。
/// ユーザー名の読み書きは [`crate::state::get_user_name`] / [`crate::state::set_user_name`]
/// （S1 #48 実装済み、`ReadFn`/`WriteFn` 互換の関数ポインタ）にそのまま委譲する。
/// 出力方式は `RenderMode::Dom` を指定する（TASK-4.4-S3、本モジュールコメント参照）。
pub fn settings_screen() -> Screen<AppState> {
    Screen::new("設定")
        .text("設定")
        .text_field("ユーザー名", get_user_name, set_user_name)
        .button(WidgetId::from_raw(1), "一覧へ戻る", go_to_list)
        .with_render_mode(RenderMode::Dom)
}

#[cfg(test)]
mod tests {
    use fandhe_core::Node;

    use super::*;
    use crate::state::Route;

    #[test]
    fn settings_screen_builds_with_at_least_one_node() {
        let screen = settings_screen();
        assert_eq!(screen.title(), "設定");
        assert!(!screen.is_empty());
    }

    /// 設定画面が `RenderMode::Dom` を指定していることを検証する（TASK-4.4-S3、
    /// `<label for>`/`<input id>` 関連付けの効果が現れる画面として選定）。
    #[test]
    fn settings_screen_uses_dom_render_mode() {
        use fandhe_core::RenderMode;

        let screen = settings_screen();
        assert_eq!(screen.render_mode(), RenderMode::Dom);
    }

    /// ノード構成・順序が設計書 2 章の確定構成
    /// （見出し `Text` → ユーザー名 `TextField` → 「一覧へ戻る」`Button`）どおりであることを検証する。
    #[test]
    fn settings_screen_has_expected_nodes_in_order() {
        let screen = settings_screen();
        assert_eq!(screen.len(), 3);

        let mut nodes = screen.iter();

        match nodes.next() {
            Some(Node::Text(text)) => assert_eq!(text, "設定"),
            other => panic!("先頭は Text バリアントであるべき: {other:?}"),
        }
        match nodes.next() {
            Some(Node::TextField { label, .. }) => assert_eq!(label, "ユーザー名"),
            other => panic!("2 番目は TextField バリアントであるべき: {other:?}"),
        }
        match nodes.next() {
            Some(Node::Button { label, .. }) => assert_eq!(label, "一覧へ戻る"),
            other => panic!("3 番目は Button バリアントであるべき: {other:?}"),
        }
        assert!(nodes.next().is_none());
    }

    /// 実画面のノードから取り出した `get`/`set` を経由してユーザー名が
    /// `AppState.settings.user_name` へ往復することを検証する
    /// （state.rs 側の関数単体テストとは異なり、`Screen` のノード経由での検証）。
    #[test]
    fn text_field_round_trips_user_name_through_state() {
        let screen = settings_screen();
        let (get, set) = screen
            .iter()
            .find_map(|node| match node {
                Node::TextField { get, set, .. } => Some((*get, *set)),
                _ => None,
            })
            .expect("ユーザー名の TextField が存在するべき");

        let mut state = AppState::default();
        assert_eq!(get(&state), "");

        set(&mut state, "太郎".to_string());

        assert_eq!(get(&state), "太郎");
        assert_eq!(state.settings.user_name, "太郎");
    }

    /// 「一覧へ戻る」ボタン押下で `route` が `Settings` から `List` へ更新されることを検証する。
    #[test]
    fn back_to_list_button_navigates_to_list_route() {
        let screen = settings_screen();
        let mut state = AppState {
            route: Route::Settings,
            ..Default::default()
        };

        let clicked = fandhe_core::click_button(&screen, "一覧へ戻る", &mut state);

        assert!(clicked);
        assert_eq!(state.route, Route::List);
    }

    /// 存在しないラベルでは `click_button` が `false` を返し、状態が変わらないことを検証する
    /// （panic を API 契約にしない方針を固定化する）。
    #[test]
    fn click_button_returns_false_for_unknown_label() {
        let screen = settings_screen();
        let mut state = AppState {
            route: Route::Settings,
            ..Default::default()
        };

        let clicked = fandhe_core::click_button(&screen, "存在しないボタン", &mut state);

        assert!(!clicked);
        assert_eq!(state.route, Route::Settings);
    }
}
