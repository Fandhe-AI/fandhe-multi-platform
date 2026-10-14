//! 画面間ナビゲーション・状態共有の統合テスト（S5 #52）。
//!
//! `fandhe-core` の公開 API（`StateStore::dispatch` / `fandhe_prototype::current_screen`）
//! のみを使い、プラットフォームレイヤ（`fandhe-egui-proxy` 等）が担う「毎フレーム
//! `current_screen` を引き直して描画する」ループを模して、一覧・詳細・設定の
//! 3 画面を横断する遷移シナリオを検証する。単一 `AppState` を経由した状態共有
//! （`items` の選択添字・メモ本文・設定値が画面をまたいで保持されること）の
//! 実証が本テストのねらい（`docs/architecture/prototype-app-design.md` 7 章）。

use fandhe_core::{Event, StateStore};
use fandhe_prototype::{AppState, Route, current_screen};

/// `dispatch` → `current_screen` 引き直しの 1 往復を行うヘルパー。
///
/// プラットフォームレイヤの実配線（`fandhe-egui-proxy::render_screen` 相当）を
/// 統合テストの粒度で再現する。
fn dispatch(store: &mut StateStore<AppState>, event: Event) -> bool {
    let screen = current_screen(store.state());
    store.dispatch(&screen, &event)
}

fn button(label: &str) -> Event {
    Event::ButtonClicked {
        label: label.to_string(),
    }
}

fn text_field(label: &str, value: &str) -> Event {
    Event::TextFieldChanged {
        label: label.to_string(),
        value: value.to_string(),
    }
}

#[test]
fn full_navigation_and_state_sharing_scenario_across_three_screens() {
    let mut store = StateStore::new(AppState::default());

    // 1. 初期状態: 一覧画面・version 0。
    assert_eq!(current_screen(store.state()).title(), "一覧");
    assert_eq!(store.version(), 0);

    // 2. 「項目を追加」→ items が 1 件増え selected が追従する。
    assert!(dispatch(&mut store, button("項目を追加")));
    assert_eq!(store.state().items.len(), 1);
    assert_eq!(store.state().selected, 0);
    assert_eq!(store.version(), 1);

    // 3. 「詳細を開く」→ route が Detail、selected は不変。
    assert!(dispatch(&mut store, button("詳細を開く")));
    assert_eq!(store.state().route, Route::Detail);
    assert_eq!(current_screen(store.state()).title(), "詳細");
    assert_eq!(store.state().selected, 0);
    assert_eq!(store.version(), 2);

    // 4. 詳細画面でメモを編集する（画面をまたぐ状態共有の起点）。
    assert!(dispatch(&mut store, text_field("メモ", "会議の議事録")));
    assert_eq!(store.state().items[0].memo, "会議の議事録");
    assert_eq!(store.version(), 3);

    // 5. 「一覧へ戻る」→ route が List に戻り、items・memo が保持されている
    //    （単一 AppState 共有の実証）。
    assert!(dispatch(&mut store, button("一覧へ戻る")));
    assert_eq!(store.state().route, Route::List);
    assert_eq!(store.state().items.len(), 1);
    assert_eq!(store.state().items[0].memo, "会議の議事録");
    assert_eq!(store.version(), 4);

    // 6. 「設定」→ route が Settings。ユーザー名を編集し、一覧へ戻った後も保持される。
    assert!(dispatch(&mut store, button("設定")));
    assert_eq!(store.state().route, Route::Settings);
    assert_eq!(current_screen(store.state()).title(), "設定");
    assert_eq!(store.version(), 5);

    assert!(dispatch(&mut store, text_field("ユーザー名", "花子")));
    assert_eq!(store.state().settings.user_name, "花子");
    assert_eq!(store.version(), 6);

    assert!(dispatch(&mut store, button("一覧へ戻る")));
    assert_eq!(store.state().route, Route::List);
    assert_eq!(store.state().settings.user_name, "花子");
    assert_eq!(store.version(), 7);

    // 7. 未知ラベルの dispatch は false を返し、version・状態を変えない。
    let version_before = store.version();
    assert!(!dispatch(&mut store, button("存在しないボタン")));
    assert_eq!(store.version(), version_before);
    assert_eq!(store.state().route, Route::List);
}

#[test]
fn select_next_and_prev_move_across_multiple_items_and_persist_after_navigation() {
    let mut store = StateStore::new(AppState::default());

    assert!(dispatch(&mut store, button("項目を追加")));
    assert!(dispatch(&mut store, button("項目を追加")));
    assert!(dispatch(&mut store, button("項目を追加")));
    assert_eq!(store.state().items.len(), 3);
    assert_eq!(store.state().selected, 2);

    // 「前へ」で選択を戻す。
    assert!(dispatch(&mut store, button("前へ")));
    assert_eq!(store.state().selected, 1);

    // 詳細画面へ遷移しても選択添字は保持される（go_to_detail は selected を変更しない）。
    assert!(dispatch(&mut store, button("詳細を開く")));
    assert_eq!(store.state().selected, 1);

    // 一覧へ戻った後も「次へ」で選択を進められる。
    assert!(dispatch(&mut store, button("一覧へ戻る")));
    assert!(dispatch(&mut store, button("次へ")));
    assert_eq!(store.state().selected, 2);
    // 末尾では据え置き。
    assert!(dispatch(&mut store, button("次へ")));
    assert_eq!(store.state().selected, 2);
}
