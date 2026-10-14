//! Phase 11（端末機能の拡充ループ、親 #1003）型サンプル「通知」（Issue #1004）。
//! タイトル欄 + 本文欄 + 「通知する」ボタン → OS へローカル通知の表示を依頼し、
//! 結果（表示した/送信した/キャンセル/失敗）を表示する 1 画面。
//! `FANDHE_PROXY_APP=notification_send` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod notification_send;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の OS 呼び出し・入力検証は
//! 本モジュール外（`fandhe-egui-proxy` の effect ドライバ、`device/notification.rs`）が
//! [`take_request`] で要求を観測し [`finish_notify`] で結果を注入する
//! （`share_text.rs` と同型の状態機械）。
//!
//! # 成功・キャンセル・失敗を混同しない理由
//!
//! `share_text`/`file_pick` と同じ再発防止方針: OS へ渡せたが表示されたかどうか
//! 確かめられない（`Submitted`）ことと、実際に表示を確認できた（`Shown`）ことを
//! 混同しない。本画面は「表示しました」と言い切らず、`SUBMITTED_STATUS` は
//! 「送信しました（表示されるかは OS の通知設定次第です）」に留める
//! （`device-bridge-contracts.md` の「6. 嘘の成功」参照）。
//!
//! ```text
//! Idle --「通知する」（タイトル空なら要求を立てず案内のみ）--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_sending--> phase=Sending
//!   --finish_notify（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 「通知する」ボタンの `WidgetId`。
const NOTIFY_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// タイトルが空のまま「通知する」を押したときの案内。
pub const EMPTY_TITLE_STATUS: &str = "タイトルを入力してください";
/// OS から実際に表示されたことを確かめられたときの案内（本 Issue の実装では
/// 到達しない。iOS 等、確認手段を持つ実装が後続 Issue で使う）。
pub const SHOWN_STATUS: &str = "通知を表示しました";
/// OS へ渡せたが表示されたかどうかは確かめられないときの案内。「表示しました」を
/// 含めない（#859 型の事故防止: 表示できたと誤解させる文言を混ぜない）。
pub const SUBMITTED_STATUS: &str = "送信しました（表示されるかは OS の通知設定次第です）";
/// 権限ダイアログを閉じただけで未決定のまま終わったときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 通知操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotificationPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS へ通知を依頼して処理中（[`mark_sending`] 済み）。
    Sending,
}

/// 通知結果（画面表示用。`fandhe_device::notification::NotifyOutcome` と 3 腕対応する。
/// `fandhe-core` は `fandhe-device` に依存しないため、写像は `fandhe-egui-proxy`
/// 側の責務にする、モジュール doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyOutcome {
    /// OS から実際に表示されたことを確かめられた。
    Shown,
    /// OS へ渡したが、表示されたかどうかは確かめられない。
    Submitted,
    /// 権限ダイアログを閉じただけで未決定のまま終わった。
    Cancelled,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRequest {
    /// [`finish_notify`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 通知のタイトル（trim・検証は effect ドライバ側の
    /// `fandhe_device::notification::NotificationRequest::new` に一本化する、SSOT 方針）。
    pub title: String,
    /// 通知の本文。
    pub body: String,
}

/// 通知画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotificationSendState {
    /// タイトル欄の入力値。
    pub title: String,
    /// 本文欄の入力値。
    pub body: String,
    /// 通知操作の進行状態。
    pub phase: NotificationPhase,
    /// 直近に立てた要求の世代番号。要求のたびに進み、完了通知が「今の要求」を
    /// 指しているかの照合に使う（モジュール doc 参照）。
    pub generation: u64,
    /// 成功・キャンセルの案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列にする。
    pub error: String,
}

/// タイトル欄の表示値（`Screen::text_field` の getter）。
pub fn get_title(state: &NotificationSendState) -> String {
    state.title.clone()
}

/// タイトル欄の入力を反映する（`Screen::text_field` の setter）。
pub fn set_title(state: &mut NotificationSendState, value: String) {
    state.title = value;
}

/// 本文欄の表示値（`Screen::text_field` の getter）。
pub fn get_body(state: &NotificationSendState) -> String {
    state.body.clone()
}

/// 本文欄の入力を反映する（`Screen::text_field` の setter）。
pub fn set_body(state: &mut NotificationSendState, value: String) {
    state.body = value;
}

/// 「通知する」ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op
/// （panic を API 契約にしない、coding-rust.md）。trim 後のタイトルが空なら
/// 要求を立てず案内だけ出す（`share_text::share` と同型）。
pub fn notify(state: &mut NotificationSendState) {
    if state.phase != NotificationPhase::Idle {
        return;
    }
    if state.title.trim().is_empty() {
        state.status = EMPTY_TITLE_STATUS.to_string();
        state.error = String::new();
        return;
    }
    state.phase = NotificationPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果表示（status/error）を残さない（`share_text::share` と同じ判断）。
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない。
/// 実際に `Sending` へ進めるのは [`mark_sending`]）。
pub fn take_request(state: &NotificationSendState) -> Option<NotificationRequest> {
    (state.phase == NotificationPhase::Requested).then(|| NotificationRequest {
        generation: state.generation,
        title: state.title.clone(),
        body: state.body.clone(),
    })
}

/// 要求を送信中（`Sending`）へ進める。`Requested` 以外では何もしない。
pub fn mark_sending(state: &mut NotificationSendState) {
    if state.phase == NotificationPhase::Requested {
        state.phase = NotificationPhase::Sending;
    }
}

/// 通知結果を反映する。`generation` は [`take_request`] が返した世代番号（呼び出し元が
/// 送信中も保持し続ける契約）。現在の世代と一致しない、または `Idle`（要求が無い）で
/// 届いた完了は無視する（`share_text::finish_share` と同型）。
pub fn finish_notify(
    state: &mut NotificationSendState,
    generation: u64,
    result: Result<NotifyOutcome, String>,
) {
    if state.phase == NotificationPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = NotificationPhase::Idle;
    match result {
        Ok(NotifyOutcome::Shown) => {
            state.status = SHOWN_STATUS.to_string();
            state.error = String::new();
        }
        Ok(NotifyOutcome::Submitted) => {
            state.status = SUBMITTED_STATUS.to_string();
            state.error = String::new();
        }
        Ok(NotifyOutcome::Cancelled) => {
            state.status = CANCELLED_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &NotificationSendState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`error_text` の `get`。`TextRole::Error` として描画される）。
pub fn error_text(state: &NotificationSendState) -> String {
    state.error.clone()
}

/// 通知画面を組み立てる。`registry.rs` の `effect` 区画から `fn(&S) -> Screen<S>` として
/// 呼ばれる契約。`Idle` のときのみボタンを出し、処理中（`Requested`/`Sending`）は
/// 二重押しを防ぐため外す（`share_text::current_screen` と同型）。
pub fn current_screen(state: &NotificationSendState) -> Screen<NotificationSendState> {
    let screen = Screen::new("通知")
        .text_field("タイトル", get_title, set_title)
        .text_field("本文", get_body, set_body)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != NotificationPhase::Idle {
        return screen;
    }
    screen.button(NOTIFY_BUTTON_ID, "通知する", notify)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_advances_generation_and_phase() {
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            ..Default::default()
        };
        notify(&mut state);
        assert_eq!(state.phase, NotificationPhase::Requested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn notify_rejects_blank_title_without_advancing() {
        let mut state = NotificationSendState {
            title: "   ".to_string(),
            ..Default::default()
        };
        notify(&mut state);
        assert_eq!(state.phase, NotificationPhase::Idle);
        assert_eq!(state.generation, 0);
        assert_eq!(state.status, EMPTY_TITLE_STATUS);
    }

    #[test]
    fn notify_rejects_blank_title_clears_previous_error() {
        let mut state = NotificationSendState {
            title: "   ".to_string(),
            error: format!("{FAILED_PREFIX}前回の失敗"),
            ..Default::default()
        };
        notify(&mut state);
        assert_eq!(state.status, EMPTY_TITLE_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn notify_clears_previous_status_and_error_on_new_request() {
        let mut state = NotificationSendState {
            title: "hello again".to_string(),
            status: SUBMITTED_STATUS.to_string(),
            error: format!("{FAILED_PREFIX}前回の失敗"),
            ..Default::default()
        };
        notify(&mut state);
        assert_eq!(state.phase, NotificationPhase::Requested);
        assert_eq!(state.status, "");
        assert_eq!(state.error, "");
    }

    #[test]
    fn notify_is_noop_while_in_flight() {
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            phase: NotificationPhase::Sending,
            generation: 1,
            ..Default::default()
        };
        notify(&mut state);
        assert_eq!(state.phase, NotificationPhase::Sending);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn take_request_returns_generation_title_and_body_only_when_requested() {
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            body: "world".to_string(),
            ..Default::default()
        };
        assert_eq!(take_request(&state), None);
        notify(&mut state);
        assert_eq!(
            take_request(&state),
            Some(NotificationRequest {
                generation: 1,
                title: "hello".to_string(),
                body: "world".to_string(),
            })
        );
        mark_sending(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn finish_notify_submitted_does_not_say_shown() {
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            ..Default::default()
        };
        notify(&mut state);
        mark_sending(&mut state);
        finish_notify(&mut state, 1, Ok(NotifyOutcome::Submitted));
        assert_eq!(state.phase, NotificationPhase::Idle);
        assert_eq!(state.status, SUBMITTED_STATUS);
        assert!(!state.status.contains("表示しました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_notify_shown_sets_status_without_error() {
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            ..Default::default()
        };
        notify(&mut state);
        mark_sending(&mut state);
        finish_notify(&mut state, 1, Ok(NotifyOutcome::Shown));
        assert_eq!(state.status, SHOWN_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_notify_cancelled_does_not_become_failure() {
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            ..Default::default()
        };
        notify(&mut state);
        mark_sending(&mut state);
        finish_notify(&mut state, 1, Ok(NotifyOutcome::Cancelled));
        assert_eq!(state.status, CANCELLED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_notify_error_sets_error_without_success_status() {
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            ..Default::default()
        };
        notify(&mut state);
        mark_sending(&mut state);
        finish_notify(
            &mut state,
            1,
            Err("この環境では通知を使えません".to_string()),
        );
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
        assert_ne!(state.status, SUBMITTED_STATUS);
    }

    #[test]
    fn finish_notify_ignores_stale_generation() {
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            ..Default::default()
        };
        notify(&mut state); // generation = 1
        mark_sending(&mut state);
        finish_notify(&mut state, 1, Ok(NotifyOutcome::Cancelled)); // Idle に戻る

        notify(&mut state); // generation = 2
        finish_notify(&mut state, 1, Ok(NotifyOutcome::Submitted));
        // 古い世代の完了は無視され、Requested のまま
        assert_eq!(state.phase, NotificationPhase::Requested);
        assert_eq!(state.status, "");
    }

    #[test]
    fn finish_notify_ignores_completion_when_idle() {
        let mut state = NotificationSendState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_notify(&mut state, 0, Ok(NotifyOutcome::Submitted));
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = NotificationSendState {
            phase: NotificationPhase::Sending,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "通知する", &mut state));
    }

    #[test]
    fn button_is_clickable_when_idle() {
        use crate::click_button;
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(click_button(&screen, "通知する", &mut state));
        assert_eq!(state.phase, NotificationPhase::Requested);
    }
}
