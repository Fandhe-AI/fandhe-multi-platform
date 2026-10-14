//! プロトタイプアプリ全体で共有する単一状態型 `AppState` とその状態アクセサ。
//!
//! `fandhe_core::ui::Node<S>` の状態アクセスは関数ポインタ（`fn(&S) -> T` /
//! `fn(&mut S, T)`）に固定され、状態型 `S` はノード単位に切り替えられない。
//! このため 3 画面（一覧・詳細・設定）すべてで同一の `AppState` を共有する構成とし、
//! 画面遷移も「状態のフィールド（`route`）を書き換える」という既存のイベント配線の
//! 特殊例として扱う（[`crate::nav`] 参照、S5 #52 が遷移ハンドラ本体を実装する）。
//!
//! 本ファイルが提供するアクセサ（`get_memo` 等）はすべて関数ポインタとして定義し、
//! クロージャは使わない。`fandhe_core::state::ReadFn` / `WriteFn`（REQ-13 / NFR-9 の
//! 型安全 DSL 方針）へそのまま代入できることを型レベルで担保するのがねらいである。

use fandhe_core::state::{ReadFn, WriteFn};

/// 現在表示中の画面を表すルーティング ID。
///
/// [`crate::nav::current_screen`] がこの値に応じて表示する `Screen<AppState>` を
/// 選択する（ディスパッチ本体は S1 #48、遷移ハンドラの実装は S5 #52）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Route {
    /// 一覧画面（S2 #49）。既定の遷移先。
    #[default]
    List,
    /// 詳細画面（S3 #50）。
    Detail,
    /// 設定画面（S4 #51）。
    Settings,
}

/// 一覧画面・詳細画面で共有される 1 項目（タスクメモ 1 件）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Item {
    /// 一覧画面に表示するタイトル。
    pub title: String,
    /// 詳細画面で編集するメモ本文。
    pub memo: String,
}

/// 設定画面が扱う設定値。フィールド追加は S4 #51 のスコープ。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// 設定画面で編集するユーザー名。
    pub user_name: String,
}

/// プロトタイプアプリ全体の共有状態。
///
/// 書き込みは（本クレートの外側から見て）常に `fandhe_core::state::StateStore::dispatch`
/// 経由の関数ポインタに限定される想定で、`&mut AppState` を公開 API から直接
/// 露出しない設計とする（REQ-13 / NFR-9 の安全網をプロトタイプでも崩さない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppState {
    /// 現在表示中の画面。
    pub route: Route,
    /// タスクメモの一覧。
    pub items: Vec<Item>,
    /// 詳細画面の対象となる `items` の添字。
    ///
    /// 範囲外アクセスによる panic を避けるため、参照側は必ず [`selected_item`] のような
    /// 境界防御済みのアクセサ経由で読む
    /// （panic を API 契約にしない、coding-rust.md のエラーハンドリング方針）。
    pub selected: usize,
    /// 設定画面が扱う設定値。
    pub settings: Settings,
}

/// `state.selected` を `items` の範囲内に収めた添字として返す。
///
/// `items` が空の場合は `0` を返す（呼び出し側は [`selected_item`] で `None` を判定する）。
fn selected_index(state: &AppState) -> usize {
    if state.items.is_empty() {
        0
    } else {
        state.selected.min(state.items.len() - 1)
    }
}

/// 現在選択中の項目への参照を返す。`items` が空の場合は `None`。
///
/// 詳細画面（S3 #50）が選択中項目のタイトル表示・メモ編集の起点として使う想定。
pub fn selected_item(state: &AppState) -> Option<&Item> {
    state.items.get(selected_index(state))
}

/// 選択中項目のメモ本文を取得する（`ReadFn<AppState, String>` 互換）。
///
/// 選択対象が存在しない（`items` が空）場合は空文字列を返す（panic を避ける）。
pub fn get_memo(state: &AppState) -> String {
    selected_item(state)
        .map(|item| item.memo.clone())
        .unwrap_or_default()
}

/// 選択中項目のメモ本文を更新する（`WriteFn<AppState, String>` 互換）。
///
/// 選択対象が存在しない場合は何もしない（panic を API 契約にしない）。
pub fn set_memo(state: &mut AppState, value: String) {
    let index = selected_index(state);
    if let Some(item) = state.items.get_mut(index) {
        item.memo = value;
    }
}

/// 設定画面のユーザー名を取得する（`ReadFn<AppState, String>` 互換）。
pub fn get_user_name(state: &AppState) -> String {
    state.settings.user_name.clone()
}

/// 設定画面のユーザー名を更新する（`WriteFn<AppState, String>` 互換）。
pub fn set_user_name(state: &mut AppState, value: String) {
    state.settings.user_name = value;
}

// アクセサが `ReadFn`/`WriteFn` へ型として代入可能であることをコンパイル時に固定化する
// （テスト（e）に対応。クロージャではなく関数ポインタであることの型レベル検証）。
const _GET_MEMO_IS_READ_FN: ReadFn<AppState, String> = get_memo;
const _SET_MEMO_IS_WRITE_FN: WriteFn<AppState, String> = set_memo;
const _GET_USER_NAME_IS_READ_FN: ReadFn<AppState, String> = get_user_name;
const _SET_USER_NAME_IS_WRITE_FN: WriteFn<AppState, String> = set_user_name;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_default_is_list() {
        assert_eq!(Route::default(), Route::List);
    }

    #[test]
    fn app_state_default_is_empty_and_valid() {
        let state = AppState::default();
        assert_eq!(state.route, Route::List);
        assert!(state.items.is_empty());
        assert_eq!(state.selected, 0);
        assert_eq!(state.settings.user_name, "");
        // 空の items でも selected_item / get_memo が panic しないことを確認する。
        assert!(selected_item(&state).is_none());
        assert_eq!(get_memo(&state), "");
    }

    #[test]
    fn get_memo_reads_selected_item() {
        let state = AppState {
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
        };

        assert_eq!(get_memo(&state), "メモ2");
    }

    #[test]
    fn set_memo_writes_selected_item() {
        let mut state = AppState {
            items: vec![Item {
                title: "1件目".to_string(),
                memo: "旧メモ".to_string(),
            }],
            selected: 0,
            ..Default::default()
        };

        set_memo(&mut state, "新メモ".to_string());

        assert_eq!(state.items[0].memo, "新メモ");
    }

    #[test]
    fn set_memo_is_no_op_when_items_is_empty() {
        let mut state = AppState::default();
        // panic せず何もしないことを確認する。
        set_memo(&mut state, "無視される".to_string());
        assert!(state.items.is_empty());
    }

    #[test]
    fn selected_out_of_range_saturates_to_last_item() {
        let state = AppState {
            items: vec![Item {
                title: "唯一の項目".to_string(),
                memo: "メモ".to_string(),
            }],
            selected: 99,
            ..Default::default()
        };

        // out-of-range な selected でも panic せず末尾要素へ丸められる。
        assert_eq!(get_memo(&state), "メモ");
    }

    #[test]
    fn get_and_set_user_name_round_trip() {
        let mut state = AppState::default();
        assert_eq!(get_user_name(&state), "");

        set_user_name(&mut state, "太郎".to_string());

        assert_eq!(get_user_name(&state), "太郎");
    }
}
