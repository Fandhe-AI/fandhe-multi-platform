//! Phase 11（端末機能の拡充ループ、親 #921）型サンプル「資格情報」（Issue #922）。
//! キー + 値の入力欄 + 「保存」「読み出し」「削除」→ OS のセキュアストア（macOS/iOS
//! Keychain 等）へ保存・読み出し・削除する 1 画面。`FANDHE_PROXY_APP=credential_vault`
//! で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から
//! 呼ばれる（`screens/mod.rs` の `pub mod credential_vault;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の保存・読み出し・削除は
//! 本モジュール外（`fandhe-egui-proxy` の effect ドライバ、
//! `device/secure_credential.rs`）が [`take_request`] で要求を観測し
//! [`finish_store`] / [`finish_load`] / [`finish_delete`] で結果を注入する。
//!
//! # 入力欄のマスク表示は本画面の範囲外（設計判断）
//!
//! `Node::TextField` にはマスク属性が無く、追加するとレンダラ横断（egui-proxy・
//! web-dom・swiftui-bridge 等 10 ファイル超）の変更になる（実装計画 §3.1）。
//! 代替として、値入力は通常の `text_field` のまま、**保存成功時に入力欄を空にする**
//! （[`finish_store`]）。読み出した値は状態側で `•` にマスクして表示し
//! （[`loaded_text`]）、「表示 / 隠す」ボタン（[`toggle_reveal`]）で切り替える。
//! DSL へのマスク付き入力欄追加は別 Issue の対象（out-of-scope-tracking.md）。
//!
//! # 要求 → 結果の一巡
//!
//! `clipboard_text.rs` と同型の同期往復（OS のセキュアストア API は同期のため
//! `http_fetch` のような二段構えは不要）。

use crate::{Screen, WidgetId};

/// 「保存」ボタンの `WidgetId`。
const STORE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「読み出し」ボタンの `WidgetId`。
const LOAD_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「削除」ボタンの `WidgetId`。
const DELETE_BUTTON_ID: WidgetId = WidgetId::from_raw(3);
/// 「表示 / 隠す」ボタンの `WidgetId`。
const REVEAL_BUTTON_ID: WidgetId = WidgetId::from_raw(4);

/// effect ドライバが `Unsupported` 相当の失敗を伝えるときに使う固定文言。
/// `fandhe-egui-proxy` 側の effect ドライバ（`device/secure_credential.rs`）と
/// 文字列で共有する（core は device に依存できないため型を共有できない、
/// `finish_*` の `Err` 判定に使う SSOT）。
pub const UNAVAILABLE_MARKER: &str = "この環境では資格情報の保存を使えません";

/// キーが空のまま操作しようとしたときの案内。
pub const EMPTY_KEY_STATUS: &str = "キーを入力してください";
/// 値が空のまま保存しようとしたときの案内。
pub const EMPTY_VALUE_STATUS: &str = "保存する値を入力してください";
/// 読み出したが値が無かったときの案内。
pub const MISSING_STATUS: &str = "保存されている値がありません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// effect ドライバへ渡す要求の種類。秘密値は `Debug` に出さない
/// （手書き実装、security.md A09。`key` は識別子のため出してよい）。
#[derive(Clone, PartialEq, Eq)]
pub enum VaultRequest {
    /// `key` に `secret` を保存する。
    Store { key: String, secret: String },
    /// `key` の値を読み出す。
    Load(String),
    /// `key` の値を削除する。
    Delete(String),
}

impl std::fmt::Debug for VaultRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store { key, secret } => f
                .debug_struct("Store")
                .field("key", key)
                .field("secret_bytes", &secret.len())
                .finish(),
            Self::Load(key) => f.debug_tuple("Load").field(key).finish(),
            Self::Delete(key) => f.debug_tuple("Delete").field(key).finish(),
        }
    }
}

/// 直近の操作結果。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Stored,
    Loaded,
    Missing,
    Deleted,
    Failed(String),
    Unavailable,
    /// 入力欄検証の案内（key/値が空）。`Failed` と異なり `FAILED_PREFIX` を付けない
    /// （effect ドライバの失敗ではなく、要求を送る前の入力チェックのため）。
    Invalid(String),
    /// OS のセキュアストア呼び出し待ち（許可ダイアログ等で無期限になりうる、
    /// モジュール doc 参照）。`store`/`load`/`delete` が要求を立てた直後に設定し、
    /// `finish_*` が結果で上書きするまで [`status_text`] に表示し続ける
    /// （Bugbot 指摘: ボタンが消えるだけで進行中であることが画面から分からない）。
    Pending,
}

/// 資格情報画面の状態。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct CredentialVaultState {
    /// キー入力欄の現在値。
    key: String,
    /// 値入力欄の現在値（保存元）。
    input: String,
    /// 「読み出し」で読み戻した値（未読み出しは `None`）。
    loaded: Option<String>,
    /// 読み出した値を生表示するか（`false` はマスク表示）。
    reveal: bool,
    /// 未処理の要求。effect ドライバが [`take_request`] で観測する。
    pending: Option<VaultRequest>,
    /// `pending` の要求が既に別スレッドへ起動済みかどうか。effect ドライバ
    /// （`fandhe-egui-proxy` 側、`device/secure_credential.rs::drive`）は毎フレーム
    /// 呼ばれるため、`pending` だけでは応答待ち中に同じ要求を毎フレーム再起動して
    /// しまう（cursor-bot High 指摘）。`take_request` は `dispatched` が立っている間
    /// `None` を返し、[`mark_dispatched`] で起動直後に立てる
    /// （`http_fetch::FetchPhase::Loading` と同じ「起動済みマーキング」の役割）。
    dispatched: bool,
    /// 直近の操作結果。
    outcome: Option<Outcome>,
}

impl std::fmt::Debug for CredentialVaultState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialVaultState")
            .field("key", &self.key)
            .field("input_chars", &self.input.chars().count())
            .field(
                "loaded_chars",
                &self.loaded.as_ref().map(|s| s.chars().count()),
            )
            .field("reveal", &self.reveal)
            .field("pending", &self.pending)
            .field("dispatched", &self.dispatched)
            .field("outcome", &self.outcome)
            .finish()
    }
}

/// キー入力欄の表示値（`Screen::text_field` の getter）。
pub fn get_key(state: &CredentialVaultState) -> String {
    state.key.clone()
}

/// キー入力欄の入力を反映する（`Screen::text_field` の setter）。
/// キーを変更したら読み出し済みの値を破棄する（別キーの秘密値が「表示」で
/// 残って見える誤表示を防ぐ、codex-review P2 指摘）。処理中（`pending` が
/// 立っている間）は no-op（応答待ち中にキーを変えると `finish_load` が
/// 別キーの結果を反映してしまう、cursor-bot Medium 指摘。ボタンは `pending` 中
/// `current_screen` から外すが入力欄は残るため、setter 側でも塞ぐ）。
pub fn set_key(state: &mut CredentialVaultState, value: String) {
    if state.pending.is_some() {
        return;
    }
    if value != state.key {
        state.loaded = None;
        state.reveal = false;
    }
    state.key = value;
}

/// 値入力欄の表示値（`Screen::text_field` の getter）。
pub fn get_input(state: &CredentialVaultState) -> String {
    state.input.clone()
}

/// 値入力欄の入力を反映する（`Screen::text_field` の setter）。処理中は no-op
/// （`set_key` と同じ理由。応答待ち中の入力変更で `finish_store` が未保存の
/// 新規入力を消してしまうのを防ぐ）。
pub fn set_input(state: &mut CredentialVaultState, value: String) {
    if state.pending.is_some() {
        return;
    }
    state.input = value;
}

/// 「保存」ボタンのハンドラ。key・値のどちらかが空なら要求を立てない。
/// 処理中（`pending` が立っている）は二重要求を防ぐため no-op。
pub fn store(state: &mut CredentialVaultState) {
    if state.pending.is_some() {
        return;
    }
    let key = state.key.trim();
    if key.is_empty() {
        state.outcome = Some(Outcome::Invalid(EMPTY_KEY_STATUS.to_string()));
        return;
    }
    if state.input.trim().is_empty() {
        state.outcome = Some(Outcome::Invalid(EMPTY_VALUE_STATUS.to_string()));
        return;
    }
    // 空判定は trim して行うが、渡す値は前後空白を含む入力そのもの
    // （前後空白も秘密値の正当な構成要素、codex-review 指摘）。
    state.pending = Some(VaultRequest::Store {
        key: key.to_string(),
        secret: state.input.clone(),
    });
    state.outcome = Some(Outcome::Pending);
}

/// 「読み出し」ボタンのハンドラ。key が空なら要求を立てない。処理中は no-op。
pub fn load(state: &mut CredentialVaultState) {
    if state.pending.is_some() {
        return;
    }
    let key = state.key.trim();
    if key.is_empty() {
        state.outcome = Some(Outcome::Invalid(EMPTY_KEY_STATUS.to_string()));
        return;
    }
    state.pending = Some(VaultRequest::Load(key.to_string()));
    state.outcome = Some(Outcome::Pending);
}

/// 「削除」ボタンのハンドラ。key が空なら要求を立てない。処理中は no-op。
pub fn delete(state: &mut CredentialVaultState) {
    if state.pending.is_some() {
        return;
    }
    let key = state.key.trim();
    if key.is_empty() {
        state.outcome = Some(Outcome::Invalid(EMPTY_KEY_STATUS.to_string()));
        return;
    }
    state.pending = Some(VaultRequest::Delete(key.to_string()));
    state.outcome = Some(Outcome::Pending);
}

/// 「表示 / 隠す」ボタンのハンドラ。読み出し済みの値が無ければ no-op。
pub fn toggle_reveal(state: &mut CredentialVaultState) {
    if state.loaded.is_some() {
        state.reveal = !state.reveal;
    }
}

/// effect ドライバが未処理の要求を観測するための getter（状態は変えない）。
/// 既に起動済み（[`mark_dispatched`] 済み）の要求は `None` を返す（応答待ち中の
/// 毎フレーム呼び出しで同じ要求を二重起動しないため、`dispatched` フィールド doc
/// 参照）。
pub fn take_request(state: &CredentialVaultState) -> Option<VaultRequest> {
    if state.dispatched {
        return None;
    }
    state.pending.clone()
}

/// effect ドライバが要求を別スレッドへ起動した直後に呼ぶ（`take_request` が
/// 同じ要求を再度返さないようにする起動済みマーキング、`dispatched` フィールド
/// doc 参照）。`CompletionSender` 経由で同期的に送る契約（`http_fetch::mark_loading`
/// と同型）。
pub fn mark_dispatched(state: &mut CredentialVaultState) {
    state.dispatched = true;
}

/// `Err` の文言が [`UNAVAILABLE_MARKER`] と一致する場合は `Outcome::Unavailable` に
/// 畳み、そうでなければ `Outcome::Failed` として保持する。
fn outcome_from_error(message: String) -> Outcome {
    if message == UNAVAILABLE_MARKER {
        Outcome::Unavailable
    } else {
        Outcome::Failed(message)
    }
}

/// 保存結果を反映する（成功時は入力欄を空にする、モジュール doc の設計判断）。
/// 要求が無い、または種別違いの完了は古い結果とみなして無視する。
pub fn finish_store(state: &mut CredentialVaultState, result: Result<(), String>) {
    if !matches!(state.pending, Some(VaultRequest::Store { .. })) {
        return;
    }
    state.pending = None;
    state.dispatched = false;
    state.outcome = Some(match result {
        Ok(()) => {
            state.input.clear();
            // 上書き保存後に古い読み出し結果（reveal 中なら平文）が画面に
            // 残らないようにする（finish_delete との非対称を解消、Bugbot 指摘）。
            state.loaded = None;
            state.reveal = false;
            Outcome::Stored
        }
        Err(message) => outcome_from_error(message),
    });
}

/// 読み出し結果を反映する（`Ok(None)` は「無い」）。表示は毎回マスクへ戻す。
/// 要求が無い、または種別違いの完了は古い結果とみなして無視する。
pub fn finish_load(state: &mut CredentialVaultState, result: Result<Option<String>, String>) {
    if !matches!(state.pending, Some(VaultRequest::Load(_))) {
        return;
    }
    state.pending = None;
    state.dispatched = false;
    state.reveal = false;
    state.outcome = Some(match result {
        Ok(Some(value)) => {
            state.loaded = Some(value);
            Outcome::Loaded
        }
        Ok(None) => {
            state.loaded = None;
            Outcome::Missing
        }
        Err(message) => {
            // 読み出し失敗時に直前の（別キーの可能性がある）秘密値を残さない
            // （codex-review P2 指摘）。
            state.loaded = None;
            outcome_from_error(message)
        }
    });
}

/// 削除結果を反映する。成功時は読み出し済みの値も破棄する。
/// 要求が無い、または種別違いの完了は古い結果とみなして無視する。
pub fn finish_delete(state: &mut CredentialVaultState, result: Result<(), String>) {
    if !matches!(state.pending, Some(VaultRequest::Delete(_))) {
        return;
    }
    state.pending = None;
    state.dispatched = false;
    state.outcome = Some(match result {
        Ok(()) => {
            state.loaded = None;
            Outcome::Deleted
        }
        Err(message) => outcome_from_error(message),
    });
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &CredentialVaultState) -> String {
    match &state.outcome {
        None => "キーと値を入力して操作してください".to_string(),
        Some(Outcome::Stored) => "保存しました".to_string(),
        Some(Outcome::Loaded) => "読み出しました".to_string(),
        Some(Outcome::Missing) => MISSING_STATUS.to_string(),
        Some(Outcome::Deleted) => "削除しました".to_string(),
        Some(Outcome::Failed(message)) => format!("{FAILED_PREFIX}{message}"),
        Some(Outcome::Unavailable) => UNAVAILABLE_MARKER.to_string(),
        Some(Outcome::Invalid(message)) => message.clone(),
        Some(Outcome::Pending) => {
            "処理中です…（OS の許可ダイアログが出ている場合があります）".to_string()
        }
    }
}

/// 読み出した値の表示（`DynamicText` の `get`）。`reveal` が `false` ならマスク
/// （`•` の文字数分）、`true` なら生値。未読み出しは案内文を出す。
pub fn loaded_text(state: &CredentialVaultState) -> String {
    match &state.loaded {
        Some(value) if state.reveal => value.clone(),
        Some(value) => "•".repeat(value.chars().count()),
        None => "「読み出し」で取得した値がここに出ます".to_string(),
    }
}

/// 資格情報画面を組み立てる。`registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。処理中はボタンを外して二重押しを防ぐ
/// （`clipboard_text::current_screen` と同型）。
pub fn current_screen(state: &CredentialVaultState) -> Screen<CredentialVaultState> {
    let screen = Screen::new("資格情報")
        .text_field("キー", get_key, set_key)
        .text_field("値", get_input, set_input)
        .dynamic_text(status_text)
        .dynamic_text(loaded_text)
        .text("値は OS のキーチェーンに保存されます");
    if state.pending.is_some() {
        return screen;
    }
    let screen = screen
        .button(STORE_BUTTON_ID, "保存", store)
        .button(LOAD_BUTTON_ID, "読み出し", load)
        .button(DELETE_BUTTON_ID, "削除", delete);
    if state.loaded.is_some() {
        screen.button(REVEAL_BUTTON_ID, "表示 / 隠す", toggle_reveal)
    } else {
        screen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_sets_pending_request_with_trimmed_key_and_untrimmed_secret() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "  demo  ".to_string());
        set_input(&mut state, "  secret-value  ".to_string());
        store(&mut state);
        // key は trim するが、秘密値は前後空白も正当な構成要素として
        // そのまま渡す（codex-review 指摘）。
        assert_eq!(
            state.pending,
            Some(VaultRequest::Store {
                key: "demo".to_string(),
                secret: "  secret-value  ".to_string(),
            })
        );
    }

    #[test]
    fn store_rejects_empty_key() {
        let mut state = CredentialVaultState::default();
        set_input(&mut state, "value".to_string());
        store(&mut state);
        assert_eq!(state.pending, None);
        assert_eq!(status_text(&state), EMPTY_KEY_STATUS);
    }

    #[test]
    fn store_rejects_empty_value() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        store(&mut state);
        assert_eq!(state.pending, None);
        assert_eq!(status_text(&state), EMPTY_VALUE_STATUS);
    }

    #[test]
    fn load_and_delete_reject_empty_key() {
        let mut state = CredentialVaultState::default();
        load(&mut state);
        assert_eq!(state.pending, None);
        assert_eq!(status_text(&state), EMPTY_KEY_STATUS);

        let mut state = CredentialVaultState::default();
        delete(&mut state);
        assert_eq!(state.pending, None);
        assert_eq!(status_text(&state), EMPTY_KEY_STATUS);
    }

    #[test]
    fn handlers_are_noop_while_request_is_in_flight() {
        let mut state = CredentialVaultState {
            pending: Some(VaultRequest::Load("x".to_string())),
            ..Default::default()
        };
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "value".to_string());
        store(&mut state);
        load(&mut state);
        delete(&mut state);
        assert_eq!(state.pending, Some(VaultRequest::Load("x".to_string())));
    }

    #[test]
    fn finish_store_clears_input_on_success() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "secret".to_string());
        store(&mut state);
        finish_store(&mut state, Ok(()));
        assert_eq!(state.pending, None);
        assert_eq!(state.input, "");
        assert_eq!(status_text(&state), "保存しました");
    }

    #[test]
    fn finish_store_clears_stale_loaded_value_and_reveal() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        load(&mut state);
        finish_load(&mut state, Ok(Some("old-secret".to_string())));
        toggle_reveal(&mut state);
        assert!(state.reveal);
        assert!(state.loaded.is_some());

        set_input(&mut state, "new-secret".to_string());
        store(&mut state);
        finish_store(&mut state, Ok(()));

        assert_eq!(
            state.loaded, None,
            "上書き保存後は古い読み出し結果を残さない"
        );
        assert!(!state.reveal);
    }

    #[test]
    fn finish_load_missing_is_reported() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        load(&mut state);
        finish_load(&mut state, Ok(None));
        assert_eq!(status_text(&state), MISSING_STATUS);
        assert_eq!(state.loaded, None);
    }

    #[test]
    fn loaded_text_is_masked_until_reveal_toggled() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        load(&mut state);
        finish_load(&mut state, Ok(Some("hunter2".to_string())));

        assert_eq!(loaded_text(&state), "•".repeat(7));
        toggle_reveal(&mut state);
        assert_eq!(loaded_text(&state), "hunter2");
        toggle_reveal(&mut state);
        assert_eq!(loaded_text(&state), "•".repeat(7));
    }

    #[test]
    fn finish_load_resets_reveal_to_masked() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        load(&mut state);
        finish_load(&mut state, Ok(Some("first".to_string())));
        toggle_reveal(&mut state);
        assert!(state.reveal);

        load(&mut state);
        finish_load(&mut state, Ok(Some("second".to_string())));
        assert!(!state.reveal, "読み直しのたびにマスクへ戻る");
    }

    #[test]
    fn finish_delete_clears_loaded_value() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        load(&mut state);
        finish_load(&mut state, Ok(Some("value".to_string())));
        assert!(state.loaded.is_some());

        delete(&mut state);
        finish_delete(&mut state, Ok(()));
        assert_eq!(state.loaded, None);
        assert_eq!(status_text(&state), "削除しました");
    }

    #[test]
    fn unavailable_marker_becomes_unavailable_outcome() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "value".to_string());
        store(&mut state);
        finish_store(&mut state, Err(UNAVAILABLE_MARKER.to_string()));
        assert_eq!(status_text(&state), UNAVAILABLE_MARKER);
    }

    #[test]
    fn other_failure_uses_failed_prefix() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "value".to_string());
        store(&mut state);
        finish_store(&mut state, Err("内部エラー".to_string()));
        assert!(status_text(&state).starts_with(FAILED_PREFIX));
    }

    #[test]
    fn store_shows_pending_status_while_in_flight() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "value".to_string());
        store(&mut state);
        assert!(matches!(state.outcome, Some(Outcome::Pending)));
        assert_ne!(status_text(&state), "キーと値を入力して操作してください");
    }

    #[test]
    fn stale_completion_is_ignored() {
        let mut state = CredentialVaultState::default();
        finish_store(&mut state, Ok(()));
        finish_load(&mut state, Ok(Some("無視される".to_string())));
        finish_delete(&mut state, Ok(()));
        assert_eq!(state.outcome, None);
        assert_eq!(state.loaded, None);
    }

    #[test]
    fn mismatched_completion_kind_is_ignored() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "value".to_string());
        store(&mut state);
        finish_load(&mut state, Ok(Some("別物".to_string())));
        assert!(matches!(state.pending, Some(VaultRequest::Store { .. })));
        assert_eq!(state.loaded, None);
    }

    #[test]
    fn debug_output_does_not_contain_secret_values() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "hunter2".to_string());
        store(&mut state);
        finish_store(&mut state, Ok(()));
        load(&mut state);
        finish_load(&mut state, Ok(Some("loaded-secret".to_string())));

        let debug = format!("{state:?}");
        assert!(!debug.contains("hunter2"));
        assert!(!debug.contains("loaded-secret"));
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = CredentialVaultState {
            pending: Some(VaultRequest::Load("x".to_string())),
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "保存", &mut state));
        assert!(!click_button(&screen, "読み出し", &mut state));
        assert!(!click_button(&screen, "削除", &mut state));
    }

    #[test]
    fn reveal_button_only_shown_after_load() {
        use crate::click_button;
        let mut state = CredentialVaultState::default();
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "表示 / 隠す", &mut state));

        set_key(&mut state, "demo".to_string());
        load(&mut state);
        finish_load(&mut state, Ok(Some("value".to_string())));
        let screen = current_screen(&state);
        assert!(click_button(&screen, "表示 / 隠す", &mut state));
        assert!(state.reveal);
    }

    #[test]
    fn key_and_input_edits_are_noop_while_request_is_in_flight() {
        let mut state = CredentialVaultState {
            key: "a".to_string(),
            input: "orig".to_string(),
            pending: Some(VaultRequest::Load("a".to_string())),
            ..Default::default()
        };
        set_key(&mut state, "b".to_string());
        set_input(&mut state, "changed".to_string());
        assert_eq!(get_key(&state), "a", "応答待ち中はキーを変更できない");
        assert_eq!(get_input(&state), "orig", "応答待ち中は値を変更できない");
    }

    #[test]
    fn take_request_returns_none_once_dispatched_until_finished() {
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        load(&mut state);
        assert!(take_request(&state).is_some());

        mark_dispatched(&mut state);
        assert_eq!(
            take_request(&state),
            None,
            "起動済みの要求は毎フレーム再起動しない"
        );

        finish_load(&mut state, Ok(Some("value".to_string())));
        assert!(!state.dispatched, "完了で起動済みフラグは解除される");
    }

    #[test]
    fn buttons_are_clickable_when_idle() {
        use crate::click_button;
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "value".to_string());
        let screen = current_screen(&state);
        assert!(click_button(&screen, "保存", &mut state));
        assert!(matches!(state.pending, Some(VaultRequest::Store { .. })));
    }
}
