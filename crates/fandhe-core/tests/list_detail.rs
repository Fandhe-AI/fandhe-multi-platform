//! `crate::screens::list_detail`（Issue #730）の結合テスト。
//!
//! 商品一覧（`VirtualList` + 行タップ）→詳細（お気に入りトグル）→戻る、の
//! 画面遷移スタック（[`fandhe_core::ScreenStack`]）と、詳細で切り替えた
//! お気に入りが一覧の行表示へ反映されること（状態共有）を、クレート外部
//! （`tests/`）から公開 API のみを用いて検証する。

use fandhe_core::screens::list_detail::{
    ListDetailState, PRODUCT_COUNT, current_screen, row_label,
};
use fandhe_core::{Event, StateStore, click_button_at, render_headless};

/// 初期状態の `current_screen` が一覧画面で、20 行すべてが未お気に入り（☆）で
/// 描画され、「戻る」は出ないことを確認する。
#[test]
fn initial_list_renders_20_rows_without_back() {
    let state = ListDetailState::default();
    assert_eq!(state.products.len(), PRODUCT_COUNT);

    let screen = current_screen(&state);
    assert_eq!(screen.title(), "一覧");

    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"(☆ 商品 01 ¥100)".to_string()));
    assert!(lines.contains(&format!(
        "(☆ 商品 {PRODUCT_COUNT:02} ¥{})",
        100 * PRODUCT_COUNT as u32
    )));
    assert!(!lines.contains(&"(戻る)".to_string()));
}

/// 5 件目（index=4）の行タップで、その商品の詳細へ遷移し `selected` が更新される
/// ことを確認する。
#[test]
fn tapping_row_pushes_detail_of_that_product() {
    let mut state = ListDetailState::default();
    let screen = current_screen(&state);
    let label = row_label(&state.products[4]);

    let handled = click_button_at(&screen, &[1, 4], &label, &mut state);
    assert!(handled);
    assert_eq!(state.selected, 4);

    let detail = current_screen(&state);
    assert_eq!(detail.title(), "詳細");
    let lines = render_headless(&detail, &state);
    assert!(lines.contains(&"商品 05".to_string()));
    assert!(lines.contains(&"¥500".to_string()));
    assert!(lines.contains(&"商品 05 の説明文です。".to_string()));
    assert!(lines.contains(&"お気に入り: ☆ 未登録".to_string()));
}

/// 状態共有の本丸: 行タップ→詳細でお気に入り切替→戻るで、一覧の該当行のみが
/// ★ に変わり他行は ☆ のまま。再度その行を開くと詳細にも反映されていることを
/// 確認する。
#[test]
fn favorite_toggle_in_detail_is_reflected_in_list_after_back() {
    let mut store = StateStore::new(ListDetailState::default());
    let label = row_label(&store.state().products[4]);

    let screen = current_screen(store.state());
    assert!(store.dispatch(
        &screen,
        &Event::ButtonClickedAt {
            label,
            path: vec![1, 4],
        },
    ));

    let detail = current_screen(store.state());
    assert!(store.dispatch(
        &detail,
        &Event::ButtonClicked {
            label: "お気に入りを切り替える".into(),
        },
    ));

    let detail = current_screen(store.state());
    assert!(store.dispatch(
        &detail,
        &Event::ButtonClicked {
            label: "戻る".into(),
        },
    ));

    let list = current_screen(store.state());
    assert_eq!(list.title(), "一覧");
    let lines = render_headless(&list, store.state());
    assert!(lines.contains(&"(★ 商品 05 ¥500)".to_string()));
    assert!(lines.contains(&"(☆ 商品 01 ¥100)".to_string()));
    assert!(lines.contains(&"(☆ 商品 06 ¥600)".to_string()));

    let label = row_label(&store.state().products[4]);
    let screen = current_screen(store.state());
    assert!(store.dispatch(
        &screen,
        &Event::ButtonClickedAt {
            label,
            path: vec![1, 4],
        },
    ));
    let detail = current_screen(store.state());
    let lines = render_headless(&detail, store.state());
    assert!(lines.contains(&"お気に入り: ★ 登録済み".to_string()));
}

/// `Event::BackRequested`（OS の戻る操作）が詳細画面では `.on_back()` ハンドラを
/// 発火して一覧へ戻し、root（一覧）では何もせず `false` を返すことを確認する。
#[test]
fn back_requested_pops_detail_but_is_no_op_on_root() {
    let mut store = StateStore::new(ListDetailState::default());
    let label = row_label(&store.state().products[0]);
    let screen = current_screen(store.state());
    store.dispatch(
        &screen,
        &Event::ButtonClickedAt {
            label,
            path: vec![1, 0],
        },
    );

    let detail_screen = current_screen(store.state());
    let version_before = store.version();
    let handled = store.dispatch(&detail_screen, &Event::BackRequested);

    assert!(handled);
    assert!(store.version() > version_before);
    assert_eq!(current_screen(store.state()).title(), "一覧");

    let root_screen = current_screen(store.state());
    let version_before_root = store.version();
    let handled_on_root = store.dispatch(&root_screen, &Event::BackRequested);

    assert!(!handled_on_root);
    assert_eq!(store.version(), version_before_root);
}

/// 範囲外 path・不一致ラベルは状態を変えず `false` を返すことを確認する
/// （panic を API 契約にしない、coding-rust.md）。
#[test]
fn out_of_range_row_and_unknown_label_are_not_handled() {
    let mut state = ListDetailState::default();
    let screen = current_screen(&state);

    assert!(!click_button_at(
        &screen,
        &[1, PRODUCT_COUNT],
        "存在しないラベル",
        &mut state,
    ));
    assert_eq!(state.selected, 0);

    let label = row_label(&state.products[0]);
    assert!(!click_button_at(&screen, &[1, 0], "違うラベル", &mut state));
    assert_eq!(state.selected, 0);
    // 正しいラベルなら発火することの対照（ここまでの呼び出しで状態が壊れていないこと）。
    assert!(click_button_at(&screen, &[1, 0], &label, &mut state));
}

/// `render_headless` は現在画面のみを描く（詳細表示中は一覧側の行が現れない）
/// ことを確認する。
#[test]
fn render_headless_draws_only_current_screen() {
    let mut store = StateStore::new(ListDetailState::default());
    let label = row_label(&store.state().products[0]);
    let screen = current_screen(store.state());
    store.dispatch(
        &screen,
        &Event::ButtonClickedAt {
            label,
            path: vec![1, 0],
        },
    );

    let detail_screen = current_screen(store.state());
    let lines = render_headless(&detail_screen, store.state());

    assert!(lines.contains(&"(戻る)".to_string()));
    assert!(!lines.iter().any(|l| l.contains("商品 02")));
}
