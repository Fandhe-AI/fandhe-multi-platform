//! M7（#724）の型サンプル「フォーム」（Issue #728）。「入力 → 送信 → 検証エラー表示 →
//! 成功表示」という実アプリ定番の流れを、`ui` の既存部品の組み合わせだけで構築できることを
//! 示す参照実装。`FANDHE_PROXY_APP=form` で `fandhe-egui-proxy` の
//! `screens/registry.rs` から呼ばれる。
//!
//! フレームワーク制約の回避（詳細は Issue #728 のコメント参照）:
//! - エラー文言・送信成功文言は `StyledText`（`Screen::error_text` / `dynamic_text`）で
//!   表示する（`get(state)` が空文字列時は空文字列を返す）。`Node::List` は `fandhe-web-dom` /
//!   `fandhe-swiftui-bridge` の双方で「動的な子要素一覧」の固定契約を持ち、`ul`/`li`
//!   （`SnapshotNode::List`）へ変換される（`ui/node.rs` の `DynamicText` ドキュメンテーション
//!   コメント参照）。0/1 件の条件付き単一メッセージをその契約に載せると、Web DOM 上で
//!   エラー文言 1 件が `<ul><li><p>…</p></li></ul>` として現れ、支援技術に対し実体の無い
//!   「リスト」を誤って公開してしまう（codex-review 指摘、PR #736 P1）。`DynamicText` /
//!   `StyledText` は全バックエンドで `Node::Text` と同格の単一テキストへ変換される既存の
//!   配線済み契約であり、この誤ったリスト化を起こさない。トレードオフとして、非エラー時は
//!   `render_headless` の出力に空文字列の行が 1 行残る（`ui/render.rs` の既知の挙動）。空の
//!   `<p>` / 空文字列 `SnapshotNode::Text` は実体の無いリスト化と異なり支援技術が実害なく
//!   無視できるため、誤ったリスト構造の公開（P1）を避ける方を優先する設計判断とする。

use crate::{Screen, WidgetId};

/// 名前が空のときのエラー文言（`submit` とテストの二重管理を避けるための定数）。
pub const NAME_ERROR: &str = "名前を入力してください";
/// メールアドレスに `@` が無いときのエラー文言。
pub const EMAIL_ERROR: &str = "メールアドレスに @ が含まれていません";
/// 規約未同意のときのエラー文言。
pub const AGREE_ERROR: &str = "規約への同意が必要です";
/// 検証を通過して送信できたときの表示文言。
pub const SUCCESS_MESSAGE: &str = "送信しました";

/// フォーム画面の状態。3 つの入力欄・同意チェックボックスと、送信時に確定する
/// 検証結果（`*_error`）・成功フラグ（`sent`）を保持する。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormState {
    /// 「名前」欄の入力値。
    pub name: String,
    /// 「メールアドレス」欄の入力値。
    pub email: String,
    /// 「メモ」欄の入力値（検証対象外）。
    pub memo: String,
    /// 規約同意チェックボックスの状態。`true` で同意済み。
    pub agreed: bool,
    /// `submit` 確定後の名前欄の検証エラー。`None` はエラーなし。
    pub name_error: Option<&'static str>,
    /// `submit` 確定後のメールアドレス欄の検証エラー。`None` はエラーなし。
    pub email_error: Option<&'static str>,
    /// `submit` 確定後の同意チェックの検証エラー。`None` はエラーなし。
    pub agree_error: Option<&'static str>,
    /// `submit` が全項目の検証を通過し送信成功と確定したかどうか。
    pub sent: bool,
}

/// 「名前」欄の表示値を返す（`Screen::text_field` の getter）。
pub fn get_name(state: &FormState) -> String {
    state.name.clone()
}

/// 「名前」欄の入力を `state.name` へ反映する（`Screen::text_field` の setter）。
pub fn set_name(state: &mut FormState, value: String) {
    state.name = value;
}

/// 「メールアドレス」欄の表示値を返す（`Screen::text_field` の getter）。
pub fn get_email(state: &FormState) -> String {
    state.email.clone()
}

/// 「メールアドレス」欄の入力を `state.email` へ反映する（`Screen::text_field` の setter）。
pub fn set_email(state: &mut FormState, value: String) {
    state.email = value;
}

/// 「メモ」欄の表示値を返す（`Screen::text_field` の getter）。
pub fn get_memo(state: &FormState) -> String {
    state.memo.clone()
}

/// 「メモ」欄の入力を `state.memo` へ反映する（`Screen::text_field` の setter）。
pub fn set_memo(state: &mut FormState, value: String) {
    state.memo = value;
}

/// 「規約に同意」チェックボックスの表示値を返す（`Screen::checkbox` の getter）。
pub fn get_agreed(state: &FormState) -> bool {
    state.agreed
}

/// 「規約に同意」チェックボックスの変更を `state.agreed` へ反映する
/// （`Screen::checkbox` の setter）。
pub fn set_agreed(state: &mut FormState, value: bool) {
    state.agreed = value;
}

/// 「送信」ボタンのハンドラ。名前・メールアドレス・同意チェックを検証し、
/// 各欄の `*_error` と `sent` を確定する（送信時にのみ検証する設計、入力中の
/// 動的再評価はしない）。
pub fn submit(state: &mut FormState) {
    state.name_error = if state.name.trim().is_empty() {
        Some(NAME_ERROR)
    } else {
        None
    };
    state.email_error = if state.email.contains('@') {
        None
    } else {
        Some(EMAIL_ERROR)
    };
    state.agree_error = if state.agreed {
        None
    } else {
        Some(AGREE_ERROR)
    };
    state.sent =
        state.name_error.is_none() && state.email_error.is_none() && state.agree_error.is_none();
}

/// 名前欄のエラー文言（`error_text` の `get`）。未エラー時は空文字列
/// （モジュール冒頭のフレームワーク制約の回避コメント参照）。
pub fn name_error_text(state: &FormState) -> String {
    error_message(state.name_error)
}

/// メールアドレス欄のエラー文言（`error_text` の `get`）。未エラー時は空文字列。
pub fn email_error_text(state: &FormState) -> String {
    error_message(state.email_error)
}

/// 同意チェックのエラー文言（`error_text` の `get`）。未エラー時は空文字列。
pub fn agree_error_text(state: &FormState) -> String {
    error_message(state.agree_error)
}

/// `Option<&'static str>` を `error_text` 用の文字列へ変換する共通ヘルパー
/// （3 つの `*_error_text` の重複を避ける）。`Screen::error_text` ビルダーメソッドと
/// 同名衝突を避けるため `error_message` と命名する。
fn error_message(error: Option<&'static str>) -> String {
    match error {
        Some(message) => message.to_string(),
        None => String::new(),
    }
}

/// 送信成功時のみ `SUCCESS_MESSAGE`、それ以外は空文字列（未表示相当）を返す
/// `DynamicText` の `get`。
pub fn status_text(state: &FormState) -> String {
    if state.sent {
        SUCCESS_MESSAGE.to_string()
    } else {
        String::new()
    }
}

/// フォーム画面を組み立てる。`registry.rs` から `FANDHE_PROXY_APP=form` で呼ばれる
/// エントリポイント。
pub fn form_screen() -> Screen<FormState> {
    Screen::new("フォーム")
        .text_field("名前", get_name, set_name)
        .error_text(name_error_text)
        .text_field("メールアドレス", get_email, set_email)
        .error_text(email_error_text)
        .text_field("メモ", get_memo, set_memo)
        .checkbox(WidgetId::from_raw(1), "規約に同意", get_agreed, set_agreed)
        .error_text(agree_error_text)
        .button(WidgetId::from_raw(2), "送信", submit)
        .dynamic_text(status_text)
}
