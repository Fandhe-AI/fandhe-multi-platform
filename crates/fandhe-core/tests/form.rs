//! `screens::form`（M7 型サンプル「フォーム」、Issue #728）のヘッドレス結合テスト。
//! 別クレートとしてコンパイルされるため、公開 API（`click_button` / `render_headless`）
//! のみを使う。

use fandhe_core::screens::form::{
    AGREE_ERROR, EMAIL_ERROR, FormState, NAME_ERROR, form_screen, set_email, set_memo, set_name,
};
use fandhe_core::state::WidgetValue;
use fandhe_core::ui::set_widget_value_at;
use fandhe_core::{WidgetId, click_button, render_headless};

/// 「規約に同意」チェックボックスのツリー上の位置（トップレベル 6 番目、index 5）。
/// `text_field × 3` + `error_text × 2`（名前・メール欄のエラー文）の後に続く
/// （`form_screen` の組み立て順、`screens/form.rs` 参照）。
const AGREE_CHECKBOX_PATH: [usize; 1] = [5];

#[test]
fn initial_render_shows_fields() {
    let screen = form_screen();
    let state = FormState::default();

    let lines = render_headless(&screen, &state);

    assert_eq!(lines[0], "# フォーム");
    assert!(lines.contains(&"[名前: ]".to_string()));
    assert!(lines.contains(&"[メールアドレス: ]".to_string()));
    assert!(lines.contains(&"[メモ: ]".to_string()));
    assert!(lines.contains(&"(送信)".to_string()));
    assert!(!lines.contains(&NAME_ERROR.to_string()));
    assert!(!lines.contains(&EMAIL_ERROR.to_string()));
    assert!(!lines.contains(&AGREE_ERROR.to_string()));
    assert!(!lines.contains(&"送信しました".to_string()));
    // codex-review 指摘（PR #736 P1）の回帰防止: エラー・成功文言は `StyledText` /
    // `DynamicText`（`Node::Text` と同格の単一テキスト）で表示され、`.list()`
    // （`ul`/`li` 契約）を誤って条件付き単一メッセージへ流用していないことを確認する。
    // 非エラー時は `StyledText` / `DynamicText` の既知の挙動として空文字列の行が残る
    // （form.rs モジュール冒頭のトレードオフ説明を参照）。
}

#[test]
fn empty_submit_shows_three_errors() {
    let screen = form_screen();
    let mut state = FormState::default();

    assert!(click_button(&screen, "送信", &mut state));

    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&NAME_ERROR.to_string()));
    assert!(lines.contains(&EMAIL_ERROR.to_string()));
    assert!(lines.contains(&AGREE_ERROR.to_string()));
    assert!(!lines.contains(&"送信しました".to_string()));
    assert!(!state.sent);
}

#[test]
fn partial_input_shows_remaining_errors() {
    let screen = form_screen();
    let mut state = FormState::default();
    set_name(&mut state, "山田 太郎".to_string());
    set_email(&mut state, "foo".to_string());

    assert!(click_button(&screen, "送信", &mut state));

    assert_eq!(state.name_error, None);
    assert!(state.email_error.is_some());
    assert!(state.agree_error.is_some());
}

#[test]
fn valid_submit_shows_success() {
    let screen = form_screen();
    let mut state = FormState::default();
    set_name(&mut state, "山田 太郎".to_string());
    set_email(&mut state, "a@example.com".to_string());
    set_memo(&mut state, "よろしくお願いします".to_string());

    assert!(set_widget_value_at(
        &screen,
        &AGREE_CHECKBOX_PATH,
        WidgetId::from_raw(1),
        &WidgetValue::Bool(true),
        &mut state,
    ));
    assert!(click_button(&screen, "送信", &mut state));

    let lines = render_headless(&screen, &state);
    assert!(!lines.contains(&NAME_ERROR.to_string()));
    assert!(!lines.contains(&EMAIL_ERROR.to_string()));
    assert!(!lines.contains(&AGREE_ERROR.to_string()));
    assert!(lines.contains(&"送信しました".to_string()));
    assert!(lines.contains(&"[名前: 山田 太郎]".to_string()));
    assert!(state.sent);
}

#[test]
fn unknown_button_is_not_handled() {
    let screen = form_screen();
    let mut state = FormState::default();

    assert!(!click_button(&screen, "存在しない", &mut state));
}
