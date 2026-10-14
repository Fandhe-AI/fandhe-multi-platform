//! Phase 11（端末機能の拡充ループ、親 #858）型サンプル「クリップボード」（Issue #859）。
//! 入力欄 + 「コピー」→ OS のクリップボードへ書き、「貼り付け」→ 読み戻して表示する
//! 往復 1 画面。`FANDHE_PROXY_APP=clipboard_text` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod clipboard_text;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の読み書きは本モジュール外
//! （`fandhe-egui-proxy` の effect ドライバ、`device/clipboard.rs`）が
//! [`take_request`] で要求を観測し [`finish_copy`] / [`finish_paste`] で結果を注入する。
//!
//! # 往復を「自アプリで書いて自アプリで読む」にしている理由
//!
//! iOS 16 以降、他アプリがコピーした内容を `UIPasteboard.general.string` で読むと
//! OS の許可アラートが出る。自アプリがコピーした内容の読み取りはアラート無しで
//! 通るため、型サンプルは「コピー → 貼り付け」の往復にしてダイアログに阻まれず
//! デモできる形にする（`docs/device/capabilities/clipboard.json` の iOS 欄）。
//! 他アプリでコピーした内容を読む場合はアラートが出る旨を [`IOS_NOTICE`] で画面に示す。
//!
//! # 要求 → 結果の一巡（二重実行しない根拠）
//!
//! ```text
//! Idle --copy/paste ボタン--> pending=Some(op) --effect が take_request で観測-->
//!   OS 呼び出し（同期） --finish_copy/finish_paste--> pending=None + 結果表示
//! ```
//!
//! effect ドライバは `fn(&S, &CompletionSender<S>)` の関数ポインタで状態を持てないが、
//! クリップボードは同期 API のため `http_fetch` の `Requested` → `Loading` 二段構えが
//! 要らない。フレーム内順序が「drain → 描画/dispatch → effect」で固定されている
//! （`fandhe-egui-proxy::app` の契約）ため、ボタン押下で立った `pending` を観測する
//! effect 呼び出しは 1 回だけで、完了の `apply`（次フレーム先頭の drain）で
//! `pending` が下りる。

use crate::{Screen, WidgetId};

/// 「コピー」ボタンの `WidgetId`。
const COPY_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「貼り付け」ボタンの `WidgetId`。
const PASTE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// iOS の許可アラートについての案内（モジュール doc 参照）。
pub const IOS_NOTICE: &str =
    "iOS 16 以降、他アプリでコピーした内容を読むときは OS の許可アラートが出ます";
/// 入力が空のままコピーしようとしたときの案内。
pub const EMPTY_INPUT_STATUS: &str = "コピーする文字列を入力してください";
/// クリップボードが空だったときの案内。
pub const EMPTY_CLIPBOARD_STATUS: &str = "クリップボードは空です";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// effect ドライバへ渡す要求の種類。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardRequest {
    /// 入力欄の文字列をクリップボードへ書く。
    Copy(String),
    /// クリップボードのテキストを読む。
    Paste,
}

/// クリップボード画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClipboardTextState {
    /// 入力欄の現在値（コピー元）。
    pub input: String,
    /// 「貼り付け」で読み戻した文字列（未読み出しは `None`）。
    pub pasted: Option<String>,
    /// 未処理の要求。effect ドライバが [`take_request`] で観測する。
    pub pending: Option<ClipboardRequest>,
    /// 直近の操作結果・案内文言。
    pub status: String,
}

/// 入力欄の表示値（`Screen::text_field` の getter）。
pub fn get_input(state: &ClipboardTextState) -> String {
    state.input.clone()
}

/// 入力欄の入力を反映する（`Screen::text_field` の setter）。
pub fn set_input(state: &mut ClipboardTextState, value: String) {
    state.input = value;
}

/// 「コピー」ボタンのハンドラ。空白のみの入力は要求を立てない。
/// 処理中（`pending` が立っている）は二重要求を防ぐため no-op
/// （panic を API 契約にしない、coding-rust.md）。
pub fn copy(state: &mut ClipboardTextState) {
    if state.pending.is_some() {
        return;
    }
    let text = state.input.trim();
    if text.is_empty() {
        state.status = EMPTY_INPUT_STATUS.to_string();
        return;
    }
    state.pending = Some(ClipboardRequest::Copy(text.to_string()));
    state.status = "コピー中…".to_string();
}

/// 「貼り付け」ボタンのハンドラ。処理中は no-op。
pub fn paste(state: &mut ClipboardTextState) {
    if state.pending.is_some() {
        return;
    }
    state.pending = Some(ClipboardRequest::Paste);
    state.status = "読み出し中…".to_string();
}

/// effect ドライバが未処理の要求を観測するための getter（状態は変えない。
/// 実際に `pending` を下ろすのは [`finish_copy`] / [`finish_paste`] の完了側）。
pub fn take_request(state: &ClipboardTextState) -> Option<ClipboardRequest> {
    state.pending.clone()
}

/// コピー結果を反映する（`Ok(())` で成功、`Err(文言)` で失敗）。
/// 要求が無い状態で届いた完了は古い結果とみなして無視する。
pub fn finish_copy(state: &mut ClipboardTextState, result: Result<(), String>) {
    if !matches!(state.pending, Some(ClipboardRequest::Copy(_))) {
        return;
    }
    state.pending = None;
    state.status = match result {
        Ok(()) => "コピーしました".to_string(),
        Err(message) => format!("{FAILED_PREFIX}{message}"),
    };
}

/// 貼り付け結果を反映する（`Ok(None)` はクリップボードが空）。
/// 要求が無い状態で届いた完了は古い結果とみなして無視する。
pub fn finish_paste(state: &mut ClipboardTextState, result: Result<Option<String>, String>) {
    if state.pending != Some(ClipboardRequest::Paste) {
        return;
    }
    state.pending = None;
    match result {
        Ok(Some(text)) => {
            state.status = format!("{} 文字読み出しました", text.chars().count());
            state.pasted = Some(text);
        }
        Ok(None) => {
            state.status = EMPTY_CLIPBOARD_STATUS.to_string();
            state.pasted = None;
        }
        Err(message) => state.status = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &ClipboardTextState) -> String {
    state.status.clone()
}

/// 読み戻した文字列の表示（`DynamicText` の `get`）。未読み出しは案内文を出す。
pub fn pasted_text(state: &ClipboardTextState) -> String {
    match &state.pasted {
        Some(text) => format!("読み出した内容: {text}"),
        None => "「貼り付け」で読み出した内容がここに出ます".to_string(),
    }
}

/// クリップボード画面を組み立てる。`registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。処理中はボタンを外して二重押しを防ぐ
/// （`http_fetch::current_screen` と同型）。
pub fn current_screen(state: &ClipboardTextState) -> Screen<ClipboardTextState> {
    let screen = Screen::new("クリップボード")
        .text_field("コピーする文字列", get_input, set_input)
        .dynamic_text(status_text)
        .dynamic_text(pasted_text)
        .text(IOS_NOTICE);
    if state.pending.is_some() {
        return screen;
    }
    screen
        .button(COPY_BUTTON_ID, "コピー", copy)
        .button(PASTE_BUTTON_ID, "貼り付け", paste)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_sets_pending_request_with_trimmed_input() {
        let mut state = ClipboardTextState::default();
        set_input(&mut state, "  コピー対象  ".to_string());
        copy(&mut state);
        assert_eq!(
            state.pending,
            Some(ClipboardRequest::Copy("コピー対象".to_string()))
        );
    }

    #[test]
    fn copy_rejects_blank_input() {
        let mut state = ClipboardTextState::default();
        set_input(&mut state, "   ".to_string());
        copy(&mut state);
        assert_eq!(state.pending, None);
        assert_eq!(state.status, EMPTY_INPUT_STATUS);
    }

    #[test]
    fn copy_is_noop_while_request_is_in_flight() {
        let mut state = ClipboardTextState {
            pending: Some(ClipboardRequest::Paste),
            ..Default::default()
        };
        set_input(&mut state, "あとから".to_string());
        copy(&mut state);
        assert_eq!(state.pending, Some(ClipboardRequest::Paste));
    }

    #[test]
    fn paste_sets_pending_request() {
        let mut state = ClipboardTextState::default();
        paste(&mut state);
        assert_eq!(state.pending, Some(ClipboardRequest::Paste));
    }

    #[test]
    fn round_trip_copy_then_paste() {
        // 「自アプリで書いて自アプリで読む」往復（モジュール doc 参照）を
        // 状態機械のレベルで固定する。
        let mut state = ClipboardTextState::default();
        set_input(&mut state, "往復する文字列".to_string());

        copy(&mut state);
        let Some(ClipboardRequest::Copy(text)) = take_request(&state) else {
            panic!("コピー要求が立っている");
        };
        finish_copy(&mut state, Ok(()));
        assert_eq!(state.pending, None);

        paste(&mut state);
        assert_eq!(take_request(&state), Some(ClipboardRequest::Paste));
        finish_paste(&mut state, Ok(Some(text)));

        assert_eq!(state.pasted, Some("往復する文字列".to_string()));
        assert!(state.status.contains('7'), "got: {}", state.status);
    }

    #[test]
    fn finish_paste_reports_empty_clipboard() {
        let mut state = ClipboardTextState::default();
        paste(&mut state);
        finish_paste(&mut state, Ok(None));
        assert_eq!(state.status, EMPTY_CLIPBOARD_STATUS);
        assert_eq!(state.pasted, None);
    }

    #[test]
    fn finish_paste_keeps_previous_value_on_error() {
        let mut state = ClipboardTextState {
            pasted: Some("前回の値".to_string()),
            ..Default::default()
        };
        paste(&mut state);
        finish_paste(&mut state, Err("読み取り拒否".to_string()));
        assert_eq!(state.pasted, Some("前回の値".to_string()));
        assert!(state.status.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn stale_completion_is_ignored() {
        // 要求が無い状態（既に処理済み・Idle）へ届いた完了は状態を上書きしない。
        let mut state = ClipboardTextState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_copy(&mut state, Ok(()));
        finish_paste(&mut state, Ok(Some("無視される".to_string())));
        assert_eq!(state.status, "変わらない");
        assert_eq!(state.pasted, None);
    }

    #[test]
    fn paste_completion_does_not_satisfy_copy_request() {
        // 種類違いの完了を取り違えない（`finish_copy` は Copy 要求のみ、
        // `finish_paste` は Paste 要求のみを受理する）。
        let mut state = ClipboardTextState::default();
        set_input(&mut state, "x".to_string());
        copy(&mut state);
        finish_paste(&mut state, Ok(Some("別物".to_string())));
        assert!(matches!(state.pending, Some(ClipboardRequest::Copy(_))));
        assert_eq!(state.pasted, None);
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = ClipboardTextState {
            pending: Some(ClipboardRequest::Paste),
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "コピー", &mut state));
        assert!(!click_button(&screen, "貼り付け", &mut state));
    }

    #[test]
    fn buttons_are_clickable_when_idle() {
        use crate::click_button;
        let mut state = ClipboardTextState::default();
        set_input(&mut state, "押せる".to_string());
        let screen = current_screen(&state);
        assert!(click_button(&screen, "コピー", &mut state));
        assert!(matches!(state.pending, Some(ClipboardRequest::Copy(_))));
    }
}
