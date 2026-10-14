//! `crate::screens::http_fetch`（Issue #748）の結合テスト。`render_headless` の行内容・
//! `click_button` を通して、状態機械（`Idle → Requested → Loading → Loaded/Failed`）と
//! 画面の組み替えが `current_screen(&state)` の実際の描画・イベント配線で成立することを
//! 検証する。端末 I/O（effect ドライバのスレッド起動）は本テストのスコープ外
//! （`crates/fandhe-egui-proxy` 側の結合テスト、Issue #748 コメント参照）。

use fandhe_core::screens::http_fetch::{
    FetchPhase, HttpFetchState, current_screen, finish_fetch, mark_loading, start_fetch,
};
use fandhe_core::{click_button, render_headless};

/// 初期状態（`Idle`）: 案内文 + 「取得」ボタンのみ。「再試行」は出ない。
#[test]
fn initial_render_shows_idle_state() {
    let state = HttpFetchState::default();
    let screen = current_screen(&state);
    let lines = render_headless(&screen, &state);

    assert_eq!(lines[0], "# HTTP 取得");
    assert!(lines.contains(&"ローカルのダミー API から一覧を取得します".to_string()));
    assert!(lines.contains(&"(取得)".to_string()));
    assert!(!lines.contains(&"(再試行)".to_string()));
}

/// 「取得」ボタンで `Requested` へ遷移し、再描画では「取得中…」のみでボタンは出ない。
#[test]
fn clicking_fetch_transitions_to_requested_and_hides_buttons() {
    let mut state = HttpFetchState::default();
    let screen = current_screen(&state);

    assert!(click_button(&screen, "取得", &mut state));
    assert_eq!(state.phase, FetchPhase::Requested);

    let screen = current_screen(&state);
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"取得中…".to_string()));
    assert!(!lines.contains(&"(取得)".to_string()));
    assert!(!lines.contains(&"(再試行)".to_string()));
}

/// `Requested | Loading` 中の再 `start_fetch` は二重取得防止のため no-op。
#[test]
fn start_fetch_is_noop_while_in_flight() {
    let mut state = HttpFetchState::default();
    start_fetch(&mut state);
    assert_eq!(state.phase, FetchPhase::Requested);

    start_fetch(&mut state);
    assert_eq!(state.phase, FetchPhase::Requested);

    mark_loading(&mut state);
    start_fetch(&mut state);
    assert_eq!(state.phase, FetchPhase::Loading);
}

/// `Requested` → `mark_loading` → `finish_fetch(Ok)` で一覧が表示され、
/// 「取得」ボタンが復帰する。
#[test]
fn mark_loading_then_finish_ok_shows_items_and_fetch_button() {
    let mut state = HttpFetchState::default();
    start_fetch(&mut state);
    mark_loading(&mut state);
    assert_eq!(state.phase, FetchPhase::Loading);

    finish_fetch(
        &mut state,
        Ok(vec![
            "りんご".to_string(),
            "みかん".to_string(),
            "ぶどう".to_string(),
        ]),
    );

    let screen = current_screen(&state);
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"3 件取得".to_string()));
    assert!(lines.contains(&"りんご".to_string()));
    assert!(lines.contains(&"みかん".to_string()));
    assert!(lines.contains(&"ぶどう".to_string()));
    assert!(lines.contains(&"(取得)".to_string()));
}

/// `finish_fetch(Err)` でエラー文言 + 「再試行」が現れ、押すと再度 `Requested` へ戻る。
#[test]
fn finish_fetch_err_shows_error_and_retry_restarts() {
    let mut state = HttpFetchState::default();
    start_fetch(&mut state);
    mark_loading(&mut state);
    finish_fetch(&mut state, Err("接続に失敗しました".to_string()));

    let screen = current_screen(&state);
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"エラー: 取得に失敗しました（接続に失敗しました）".to_string()));
    assert!(lines.contains(&"(再試行)".to_string()));
    assert!(!lines.contains(&"(取得)".to_string()));

    assert!(click_button(&screen, "再試行", &mut state));
    assert_eq!(state.phase, FetchPhase::Requested);
}

/// `Idle`（`Loading` 以外）で届いた `finish_fetch` は古い完了とみなして無視される。
#[test]
fn finish_fetch_is_ignored_when_not_loading() {
    let mut state = HttpFetchState::default();
    finish_fetch(&mut state, Ok(vec!["無視される".to_string()]));
    assert_eq!(state.phase, FetchPhase::Idle);
}
