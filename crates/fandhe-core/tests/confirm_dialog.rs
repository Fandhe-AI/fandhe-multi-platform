//! `crate::screens::confirm_dialog`（Issue #729 / #739）の結合テスト。`render_headless` の
//! 行内容・`click_button_at`（動的生成子内のボタン解決）・`request_back`
//! （`.on_back()`）を通して、「削除 → 確認 → キャンセル/削除/Esc」の各経路が
//! `confirm_dialog_screen(&state)` の実際の描画・イベント配線で成立することを検証する。
//!
//! Issue #739 で「一覧区画 ⇔ 確認区画」を排他的に切り替える `nav` 方式（PR #737）を
//! 「一覧区画は常時描画、その後ろに `Node::Widget(DialogWidget)` を重ねる」方式へ
//! 置き換えた。トップレベルノードの並びは常に
//! `[Text("メモ一覧"), List(expand_memo_rows), Widget(DialogWidget)]` の 3 件で固定され
//! （`pending` の値に関わらず一覧・ダイアログの両方が常に存在する）、一覧側の削除ボタンの
//! path（`[1, i]`）はダイアログの開閉に影響されない。確認パネル内のボタン
//! （キャンセル / 削除）は `path = [2, 1]` / `[2, 2]`（トップレベル 2 番目の Widget →
//! `DialogWidget::children(state)` の列挙 index。0 番目は確認文言の `Node::Text`、
//! PR #792 codex-review 指摘 B で `body` から展開可能な子ノードへ戻したため）で
//! 解決する。
//!
//! `confirm_dialog_screen` は状態を都度読み直す `fn(&S) -> Screen<S>` 契約
//! （`screens/list_detail.rs::current_screen` と同型）のため、状態を変える dispatch の
//! たびに `confirm_dialog_screen(&state)` を呼び直す。

use fandhe_core::screens::confirm_dialog::{
    ConfirmDialogState, confirm_delete, confirm_dialog_screen, request_delete,
};
use fandhe_core::{click_button_at, render_headless, request_back};

/// トップレベル 2 番目（text, list に続く）の `Node::Widget(DialogWidget)` の path index。
const DIALOG_TOP_INDEX: usize = 2;

/// 初期状態でメモ 3 件が一覧表示され、確認パネルのボタンは現れないことを検証する。
#[test]
fn initial_render_lists_three_memos() {
    let state = ConfirmDialogState::default();
    let screen = confirm_dialog_screen(&state);
    let lines = render_headless(&screen, &state);

    assert_eq!(lines[0], "# メモ");
    assert!(lines.contains(&"メモ 1".to_string()));
    assert!(lines.contains(&"メモ 2".to_string()));
    assert!(lines.contains(&"メモ 3".to_string()));
    assert_eq!(lines.iter().filter(|l| l.as_str() == "(削除)").count(), 3);
    assert!(!lines.contains(&"(キャンセル)".to_string()));
}

/// 2 番目のメモ（展開順 index=3。i=0: Text+Btn, i=1: Text+Btn が続く）の「削除」ボタンで
/// 確認パネルが現れ、背後の一覧（`memos`・削除ボタン）は変わらず残ることを検証する
/// （Issue #739: 一覧を透過表示したまま前面にパネルを重ねる構成）。
#[test]
fn delete_button_opens_confirmation() {
    let mut state = ConfirmDialogState::default();
    let screen = confirm_dialog_screen(&state);

    assert!(click_button_at(&screen, &[1, 3], "削除", &mut state));
    assert_eq!(state.pending, Some(1));
    assert_eq!(state.memos.len(), 3);

    let screen = confirm_dialog_screen(&state);
    let lines = render_headless(&screen, &state);
    // 確認文言のヘッドレス出力（PR #792 codex-review 指摘 B の回帰検証）。
    assert!(lines.contains(&"「メモ 2」を削除しますか？".to_string()));
    assert!(lines.contains(&"(キャンセル)".to_string()));
    // 背後の一覧行はパネル表示中も出力され続ける（一覧側の削除ボタンは 3 件のまま）。
    assert!(lines.contains(&"メモ 2".to_string()));
    assert_eq!(lines.iter().filter(|l| l.as_str() == "(削除)").count(), 4);
}

/// 確認パネルの「キャンセル」を押すとパネルが消え、`memos` は変わらない。
#[test]
fn cancel_keeps_memo() {
    let mut state = ConfirmDialogState::default();
    request_delete(&mut state, 1);
    let screen = confirm_dialog_screen(&state);

    assert!(click_button_at(
        &screen,
        &[DIALOG_TOP_INDEX, 1],
        "キャンセル",
        &mut state
    ));
    assert_eq!(state.pending, None);
    assert_eq!(state.memos.len(), 3);

    let screen = confirm_dialog_screen(&state);
    let lines = render_headless(&screen, &state);
    assert!(!lines.contains(&"(キャンセル)".to_string()));
}

/// 確認パネルの「削除」を押すと該当メモが消え、パネルも閉じる。
#[test]
fn confirm_removes_memo() {
    let mut state = ConfirmDialogState::default();
    request_delete(&mut state, 1);
    let screen = confirm_dialog_screen(&state);

    assert!(click_button_at(
        &screen,
        &[DIALOG_TOP_INDEX, 2],
        "削除",
        &mut state
    ));
    assert_eq!(state.pending, None);
    assert_eq!(
        state.memos,
        vec!["メモ 1".to_string(), "メモ 3".to_string()]
    );

    let screen = confirm_dialog_screen(&state);
    let lines = render_headless(&screen, &state);
    assert_eq!(lines.iter().filter(|l| l.as_str() == "(削除)").count(), 2);
    assert!(!lines.contains(&"(キャンセル)".to_string()));
}

/// `.on_back()`（Esc / Android バックキー相当）でも確認パネルが閉じ、`memos` は変わらない。
#[test]
fn back_request_cancels_confirmation() {
    let mut state = ConfirmDialogState::default();
    request_delete(&mut state, 0);
    let screen = confirm_dialog_screen(&state);

    assert!(request_back(&screen, &mut state));
    assert_eq!(state.pending, None);
    assert_eq!(state.memos.len(), 3);
}

/// 確認非表示（`pending == None`）では `.on_back()` が未登録のため `request_back` は
/// 何も変更せず `false` を返し、上位の画面遷移・アプリ終了へそのまま渡ることを検証する
/// （PR #737 レビュー P1 指摘 2 の再現ケース: 修正前は `pending == None` でも
/// `request_back` が `true` を返していた）。
#[test]
fn back_request_is_no_op_when_not_confirming() {
    let mut state = ConfirmDialogState::default();
    let screen = confirm_dialog_screen(&state);

    assert!(!request_back(&screen, &mut state));
    assert_eq!(state.pending, None);
    assert_eq!(state.memos.len(), 3);
}

/// 確認パネル表示中も一覧区画は常に描画され、ヘッドレスの `click_button_at`
/// （path 解決）では一覧側の削除ボタンの path 自体は引き続き解決できることを検証する
/// （Issue #739: `nav` 方式から「背後の一覧を残す」方式への変更に伴う仕様変更。
/// 旧仕様「確認表示中は一覧側の削除ボタンが消える」（PR #737 P1 指摘 1）は
/// 本方式では成立しない: 背後を透過表示するには一覧ノード自体が常時存在する
/// 必要があるため）。ただし path 解決後に呼ばれる `request_delete` ハンドラは
/// 確認中（`pending.is_some()`）を検知して無視するため、`pending` は上書きされず、
/// 続く確認操作は最初に選んだメモに対して行われる（PR #792 codex-review 2 巡目
/// P1 指摘: egui の backdrop は `StateStore::dispatch` 経由の操作を防がないため、
/// コア側のハンドラで背景操作を拒否する契約に変更）。
#[test]
fn confirming_ignores_other_list_delete_via_handler_guard() {
    let mut state = ConfirmDialogState::default();
    request_delete(&mut state, 0);
    let screen = confirm_dialog_screen(&state);

    // 「(削除)」は一覧側 3 件 + パネル側 1 件の計 4 件。
    let lines = render_headless(&screen, &state);
    assert_eq!(lines.iter().filter(|l| l.as_str() == "(削除)").count(), 4);
    assert!(lines.contains(&"メモ 2".to_string()));

    // 一覧側の展開順 path（[1, 3] は 2 番目メモの削除ボタン）は解決できるが、
    // ハンドラが確認中を検知して無視するため `pending` は Some(0) のまま。
    assert!(click_button_at(&screen, &[1, 3], "削除", &mut state));
    assert_eq!(state.pending, Some(0));

    // 続く確認パネル内「削除」は最初に選んだメモ 1（index 0）を消し、
    // 誤ってメモ 2（index 1）が消えないことを確認する。
    let screen = confirm_dialog_screen(&state);
    assert!(click_button_at(
        &screen,
        &[DIALOG_TOP_INDEX, 2],
        "削除",
        &mut state
    ));
    assert_eq!(state.pending, None);
    assert_eq!(
        state.memos,
        vec!["メモ 2".to_string(), "メモ 3".to_string()]
    );
}

/// 3 件すべて削除すると一覧が空になり案内文が出ることを検証する。
#[test]
fn deleting_all_shows_empty_message() {
    let mut state = ConfirmDialogState::default();

    // 削除のたびに残り件数が減るため、常に先頭（展開順 index=1、i=0 の削除ボタン）を押す。
    for _ in 0..3 {
        let screen = confirm_dialog_screen(&state);
        assert!(click_button_at(&screen, &[1, 1], "削除", &mut state));
        let screen = confirm_dialog_screen(&state);
        assert!(click_button_at(
            &screen,
            &[DIALOG_TOP_INDEX, 2],
            "削除",
            &mut state
        ));
    }

    assert!(state.memos.is_empty());
    let screen = confirm_dialog_screen(&state);
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"メモはありません".to_string()));
}

/// 範囲外 index は無視され panic しない（`request_delete` / `confirm_delete` の
/// エラーハンドリング方針、coding-rust.md 準拠）。
#[test]
fn out_of_range_index_is_ignored() {
    let mut state = ConfirmDialogState::default();

    request_delete(&mut state, 99);
    assert_eq!(state.pending, None);

    state.pending = Some(5);
    confirm_delete(&mut state);
    assert_eq!(state.memos.len(), 3);
}

/// `memos` が空で `pending` だけ範囲外 index を指す状態（`request_delete` を経由せず
/// 直接構築した不整合な状態）では確認パネルは閉状態のまま扱われ、`.on_back()` も
/// 未登録のままであることを検証する（PR #737 レビュー P1 指摘 3 の再現ケース: 修正前は
/// `pending.is_some()` だけで分岐していたため、内容のない確認区画が出た上に
/// `.on_back(cancel_delete)` が登録され、Esc / Android バックキーが上位へ渡らなかった）。
#[test]
fn out_of_range_pending_falls_back_to_closed_dialog_without_back_handler() {
    let mut state = ConfirmDialogState {
        memos: vec![],
        pending: Some(0),
    };
    let screen = confirm_dialog_screen(&state);
    let lines = render_headless(&screen, &state);

    assert!(lines.contains(&"メモはありません".to_string()));
    assert!(!lines.contains(&"(キャンセル)".to_string()));
    assert!(!request_back(&screen, &mut state));
    assert_eq!(state.pending, Some(0));
}
