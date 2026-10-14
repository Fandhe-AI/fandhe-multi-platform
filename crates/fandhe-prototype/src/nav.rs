//! ルーティングディスパッチ（`Route` → 表示する `Screen<AppState>`）と、
//! 画面間ナビゲーション・状態共有を担う遷移ハンドラ本体（S5 #52）。
//!
//! `fandhe_core::ui::Node<S>` には専用のナビゲーション機構がなく、本クレートでは
//! 画面遷移を「ボタン押下ハンドラ（`fn(&mut AppState)`）が `AppState::route` を
//! 書き換える → `StateStore::version` が進み再描画対象になる → プラットフォーム側が
//! 都度 [`current_screen`] を呼び直して表示画面を切り替える」という、既存の
//! イベント→状態更新の配線をそのまま利用する形で実現する（fandhe-core への変更・
//! 新規ナビゲーション機構の追加を避け、共通コード比率の計測にも有利な設計判断）。
//!
//! 遷移ハンドラ（[`go_to_list`] / [`go_to_detail`] / [`go_to_settings`] /
//! [`select_next`] / [`select_prev`]）のシグネチャ・責務は
//! `docs/architecture/prototype-app-design.md` 4 章で確定済みで、本モジュールが
//! 唯一の実装本体（S5 #52）である。各画面（[`crate::screens`]）はこれらを
//! `fn(&mut AppState)` の関数ポインタとしてそのまま `Screen::button` へ渡し、
//! モジュール内に重複定義しない（S2〜S4 の暫定ローカルハンドラはここへ集約済み）。
//! `AppState` を横断する状態共有（`items` の選択添字・メモ本文等）は
//! すべて単一の `AppState` を経由するため、遷移そのものはこれらのフィールドを
//! 変更しない（`go_to_detail` が `selected` を保持する等）。

use fandhe_core::Screen;

use crate::screens::{detail_screen, list_screen, settings_screen};
use crate::state::{AppState, Route};

/// 一覧画面へ遷移する。`state.route` を `Route::List` へ書き換えるのみで、
/// `items` / `selected` / `settings` 等の他フィールドは変更しない
/// （画面をまたいだ状態共有を壊さないための契約）。
///
/// 詳細画面・設定画面の「一覧へ戻る」ボタンから呼ばれる。
pub fn go_to_list(state: &mut AppState) {
    state.route = Route::List;
}

/// 詳細画面へ遷移する。`selected` は変更しない（一覧画面での選択をそのまま
/// 詳細画面へ引き継ぐための設計判断、`docs/architecture/prototype-app-design.md` 4 章）。
///
/// 一覧画面の「詳細を開く」ボタンから呼ばれる。
pub fn go_to_detail(state: &mut AppState) {
    state.route = Route::Detail;
}

/// 設定画面へ遷移する。
///
/// 一覧画面の「設定」ボタンから呼ばれる。
pub fn go_to_settings(state: &mut AppState) {
    state.route = Route::Settings;
}

/// 選択添字を 1 つ進める（`items` の末尾・空の場合は据え置き、panic しない）。
///
/// 一覧画面の「次へ」ボタンから呼ばれる。
pub fn select_next(state: &mut AppState) {
    if state.items.is_empty() {
        return;
    }
    let last = state.items.len() - 1;
    if state.selected < last {
        state.selected += 1;
    }
}

/// 選択添字を 1 つ戻す（`saturating_sub` により先頭では据え置き、panic しない）。
///
/// 一覧画面の「前へ」ボタンから呼ばれる。
pub fn select_prev(state: &mut AppState) {
    state.selected = state.selected.saturating_sub(1);
}

/// `state.route` に応じて表示すべき画面を返す。
///
/// プラットフォームレイヤ（`fandhe-egui-proxy` 等）が状態更新のたびに呼び直し、
/// 返された `Screen<AppState>` を描画する想定の入口関数。各画面本体の実装は
/// [`crate::screens`] の各サブモジュールを参照。
pub fn current_screen(state: &AppState) -> Screen<AppState> {
    match state.route {
        Route::List => list_screen(),
        Route::Detail => detail_screen(),
        Route::Settings => settings_screen(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_screen_dispatches_list_route_to_list_screen() {
        let state = AppState {
            route: Route::List,
            ..Default::default()
        };
        assert_eq!(current_screen(&state).title(), "一覧");
    }

    #[test]
    fn current_screen_dispatches_detail_route_to_detail_screen() {
        let state = AppState {
            route: Route::Detail,
            ..Default::default()
        };
        assert_eq!(current_screen(&state).title(), "詳細");
    }

    #[test]
    fn current_screen_dispatches_settings_route_to_settings_screen() {
        let state = AppState {
            route: Route::Settings,
            ..Default::default()
        };
        assert_eq!(current_screen(&state).title(), "設定");
    }

    /// 画面別の出力方式（TASK-4.4-S3 / Issue #81）を固定化する。設定画面のみ
    /// `RenderMode::Dom`（`TextField` フォームを含み a11y 効果が現れるため）を指定し、
    /// 一覧・詳細は既定の `RenderMode::Canvas` のままであることを、
    /// `current_screen` のディスパッチ経由で（画面個別実装を経由せず）検証する。
    #[test]
    fn current_screen_uses_dom_render_mode_only_for_settings() {
        use fandhe_core::RenderMode;

        let list_state = AppState {
            route: Route::List,
            ..Default::default()
        };
        let detail_state = AppState {
            route: Route::Detail,
            ..Default::default()
        };
        let settings_state = AppState {
            route: Route::Settings,
            ..Default::default()
        };

        assert_eq!(
            current_screen(&list_state).render_mode(),
            RenderMode::Canvas
        );
        assert_eq!(
            current_screen(&detail_state).render_mode(),
            RenderMode::Canvas
        );
        assert_eq!(
            current_screen(&settings_state).render_mode(),
            RenderMode::Dom
        );
    }

    // 以下、遷移ハンドラ 5 本の単体テスト（list.rs の暫定実装から移設・境界網羅）。

    #[test]
    fn go_to_list_sets_route_to_list() {
        let mut state = AppState {
            route: Route::Detail,
            ..Default::default()
        };
        go_to_list(&mut state);
        assert_eq!(state.route, Route::List);
    }

    #[test]
    fn go_to_detail_sets_route_without_changing_selected() {
        let mut state = AppState {
            selected: 3,
            ..Default::default()
        };
        go_to_detail(&mut state);
        assert_eq!(state.route, Route::Detail);
        assert_eq!(state.selected, 3);
    }

    #[test]
    fn go_to_settings_sets_route() {
        let mut state = AppState::default();
        go_to_settings(&mut state);
        assert_eq!(state.route, Route::Settings);
    }

    #[test]
    fn select_next_advances_within_bounds() {
        use crate::state::Item;

        let mut state = AppState {
            items: vec![Item::default(), Item::default(), Item::default()],
            selected: 0,
            ..Default::default()
        };

        select_next(&mut state);
        assert_eq!(state.selected, 1);
        select_next(&mut state);
        assert_eq!(state.selected, 2);
        // 末尾では据え置き
        select_next(&mut state);
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn select_next_is_no_op_when_items_is_empty() {
        let mut state = AppState::default();
        select_next(&mut state);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn select_prev_retreats_within_bounds() {
        use crate::state::Item;

        let mut state = AppState {
            items: vec![Item::default(), Item::default()],
            selected: 1,
            ..Default::default()
        };

        select_prev(&mut state);
        assert_eq!(state.selected, 0);
        // 先頭では据え置き（saturating_sub）
        select_prev(&mut state);
        assert_eq!(state.selected, 0);
    }
}
