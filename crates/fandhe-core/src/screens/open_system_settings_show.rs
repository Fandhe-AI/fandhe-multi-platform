//! Phase 11（端末機能の拡充ループ、親 #171）型サンプル「OS の設定画面を開く」（Issue #798）。
//! 自アプリの設定・通知の設定を開く 2 ボタンだけの 1 画面。
//! `FANDHE_PROXY_APP=open_system_settings_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/
//! registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod open_system_settings_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。OS 呼び出しは `fandhe-egui-proxy` の effect ドライバ
//! （`device/open_system_settings.rs`）が [`take_request`] で要求を観測して行い、[`finish_open`] で
//! 結果を注入する。本画面は二重押し防止と文言だけを持ち、全対象 OS（Windows・iOS・Android）の
//! 文言をここで決める（後続の OS 実装は画面を触らない）。
//!
//! # 結果を混同しない理由
//!
//! 「OS に依頼しただけ」「OS が開かなかった」「この OS では開けない頁」「非対応」を黙って
//! 「設定画面を開きました」に丸めると、利用者は開いた・設定したと誤解する。どの文言も
//! 開いた・変更したとは言わない。OS が判定した結果は `status`、内部失敗は `error`
//! （`TextRole::Error`）へ書き分ける。世代番号（`generation`）で古い完了や `Idle` で届いた完了を
//! 無視する（`compose_mail_sms` と同じ判断）。

use crate::{Screen, WidgetId};

/// OS に依頼しただけのときの案内（開いた・変更したとは言わない）。
pub const HANDED_OFF_STATUS: &str =
    "OS に設定画面を開くよう依頼しました（開いたか・設定を変えたかは確認できません）";
/// OS が開かなかったときの案内（失敗扱いにしない）。
pub const NOT_OPENED_STATUS: &str = "OS がこの設定画面を開きませんでした";
/// この OS では開けない頁のときの案内。
pub const PAGE_UNAVAILABLE_STATUS: &str = "この OS ではこの設定画面を開けません";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では設定画面を開けません";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 2 つの頁の説明。Windows の通知は OS 全体の頁であることを添える。
pub const NOTE_TEXT: &str = "自アプリの設定と通知の設定を OS に開いてもらいます。Windows の通知は OS 全体の通知設定で、このアプリだけの頁ではありません";
/// 「アプリの設定を開く」ボタンの文言。
pub const APP_BUTTON_LABEL: &str = "アプリの設定を開く";
/// 「通知の設定を開く」ボタンの文言。
pub const NOTIFICATIONS_BUTTON_LABEL: &str = "通知の設定を開く";

const APP_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const NOTIFICATIONS_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 開く頁（`fandhe_device::open_system_settings::SettingsPage` に対応。写像は proxy 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTarget {
    /// 自アプリの設定。
    App,
    /// 通知の設定。
    Notifications,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenSettingsPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_opening`] 済み）。
    Opening,
}

/// OS が判定した結果（`OpenSettingsOutcome` に非対応を加えたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// OS に依頼しただけ。
    HandedOff,
    /// OS が開かなかった。
    NotOpened,
    /// この OS では開けない頁。
    PageUnavailable,
    /// この環境では非対応。
    Unsupported,
}

/// 失敗（画面には固定文言だけを出す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenSettingsFailure {
    /// 内部失敗。
    Failed(String),
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenSettingsRequestView {
    /// [`finish_open`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 開く頁。
    pub target: SettingsTarget,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenSystemSettingsState {
    /// 進行状態。
    pub phase: OpenSettingsPhase,
    /// 直近に押された頁。
    pub target: Option<SettingsTarget>,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

fn request(state: &mut OpenSystemSettingsState, target: SettingsTarget) {
    if state.phase != OpenSettingsPhase::Idle {
        return;
    }
    state.phase = OpenSettingsPhase::Requested;
    state.target = Some(target);
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// 「アプリの設定を開く」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn open_app_settings(state: &mut OpenSystemSettingsState) {
    request(state, SettingsTarget::App);
}

/// 「通知の設定を開く」ボタンのハンドラ。`Idle` 以外は no-op。
pub fn open_notification_settings(state: &mut OpenSystemSettingsState) {
    request(state, SettingsTarget::Notifications);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &OpenSystemSettingsState) -> Option<OpenSettingsRequestView> {
    if state.phase != OpenSettingsPhase::Requested {
        return None;
    }
    Some(OpenSettingsRequestView {
        generation: state.generation,
        target: state.target?,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_opening(state: &mut OpenSystemSettingsState) {
    if state.phase == OpenSettingsPhase::Requested {
        state.phase = OpenSettingsPhase::Opening;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_open(
    state: &mut OpenSystemSettingsState,
    generation: u64,
    result: Result<CoreOutcome, OpenSettingsFailure>,
) {
    if state.phase == OpenSettingsPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = OpenSettingsPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::HandedOff) => state.status = HANDED_OFF_STATUS.to_string(),
        Ok(CoreOutcome::NotOpened) => state.status = NOT_OPENED_STATUS.to_string(),
        Ok(CoreOutcome::PageUnavailable) => state.status = PAGE_UNAVAILABLE_STATUS.to_string(),
        Ok(CoreOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(OpenSettingsFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &OpenSystemSettingsState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &OpenSystemSettingsState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外し、同時に 2 件出さない。
pub fn current_screen(state: &OpenSystemSettingsState) -> Screen<OpenSystemSettingsState> {
    let screen = Screen::new("OS の設定画面を開く")
        .text(NOTE_TEXT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != OpenSettingsPhase::Idle {
        return screen;
    }
    screen
        .button(APP_BUTTON_ID, APP_BUTTON_LABEL, open_app_settings)
        .button(
            NOTIFICATIONS_BUTTON_ID,
            NOTIFICATIONS_BUTTON_LABEL,
            open_notification_settings,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opening(target: SettingsTarget) -> OpenSystemSettingsState {
        let mut s = OpenSystemSettingsState::default();
        request(&mut s, target);
        mark_opening(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, OpenSettingsFailure>) -> (String, String) {
        let mut s = opening(SettingsTarget::App);
        let g = s.generation;
        finish_open(&mut s, g, r);
        assert_eq!(s.phase, OpenSettingsPhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn five_results_are_distinct_and_none_claims_opened_or_changed() {
        let all = [
            shown(Ok(CoreOutcome::HandedOff)),
            shown(Ok(CoreOutcome::NotOpened)),
            shown(Ok(CoreOutcome::PageUnavailable)),
            shown(Ok(CoreOutcome::Unsupported)),
            shown(Err(OpenSettingsFailure::Failed("x".into()))),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
            let text = format!("{}{}", a.0, a.1);
            assert!(!text.contains("設定しました") && !text.contains("変更しました"));
            assert!(!text.contains("開きました"));
        }
    }

    #[test]
    fn non_failures_do_not_go_to_error() {
        for o in [
            CoreOutcome::HandedOff,
            CoreOutcome::NotOpened,
            CoreOutcome::PageUnavailable,
            CoreOutcome::Unsupported,
        ] {
            let (status, error) = shown(Ok(o));
            assert!(!status.is_empty() && error.is_empty());
        }
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = opening(SettingsTarget::Notifications);
        let g = s.generation + 1;
        finish_open(&mut s, g, Ok(CoreOutcome::HandedOff));
        assert_eq!(s.phase, OpenSettingsPhase::Opening);
        assert!(s.status.is_empty());

        let mut idle = OpenSystemSettingsState::default();
        finish_open(&mut idle, 0, Ok(CoreOutcome::HandedOff));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn double_press_is_noop_and_take_request_matches_target() {
        let mut s = OpenSystemSettingsState::default();
        assert!(take_request(&s).is_none());
        open_notification_settings(&mut s);
        let r = take_request(&s).unwrap();
        assert_eq!(r.target, SettingsTarget::Notifications);
        open_app_settings(&mut s);
        assert_eq!(s.target, Some(SettingsTarget::Notifications));
        assert_eq!(s.generation, 1);
        mark_opening(&mut s);
        assert!(take_request(&s).is_none());
        open_app_settings(&mut s);
        assert_eq!(s.phase, OpenSettingsPhase::Opening);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn buttons_hidden_while_busy() {
        let idle = current_screen(&OpenSystemSettingsState::default());
        let busy = current_screen(&opening(SettingsTarget::App));
        assert!(format!("{idle:?}").contains(NOTIFICATIONS_BUTTON_LABEL));
        assert!(!format!("{busy:?}").contains(NOTIFICATIONS_BUTTON_LABEL));
    }
}
