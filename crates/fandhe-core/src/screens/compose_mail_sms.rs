//! Phase 11（端末機能の拡充ループ、親 #1142）型サンプル「メール・SMS の作成」（Issue #1384）。
//! 固定の宛先・件名・本文で OS の作成画面を開く 2 ボタンだけの 1 画面。
//! `FANDHE_PROXY_APP=compose_mail_sms` で選ばれ、`crates/fandhe-egui-proxy/src/screens/
//! registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod compose_mail_sms;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。入力の検査と OS 呼び出しは `fandhe-egui-proxy` の effect ドライバ
//! （`device/compose_mail_sms.rs`）が [`take_request`] で要求を観測して行い、[`finish_compose`] で
//! 結果を注入する。本画面は二重押し防止と文言だけを持つ。
//!
//! # 結果を混同しない理由
//!
//! 「OS に依頼しただけ」「送信待ちに入った」「保存した」「キャンセル」「扱うアプリが無い」を黙って
//! 「送信しました」に丸めると、利用者は送られたと誤解する。どの文言も送信完了を言わない。
//! OS が判定した正常な結果は `status`、入力不正・失敗は `error`（`TextRole::Error`）へ書き分ける。
//! 世代番号（`generation`）で古い完了や `Idle` で届いた完了を無視する（`calendar_add` と同じ判断）。

use crate::{Screen, WidgetId};

/// 固定の宛先（RFC 2606 の予約ドメイン。実在の宛先へ誤送信しない）。
pub const MAIL_TO: &str = "someone@example.invalid";
/// 固定の件名。URL の区切りになる記号と日本語を含め、OS 側で崩れないか目視で確かめる。
pub const MAIL_SUBJECT: &str = "テスト & 確認 = 1? #2";
/// 固定のメール本文（改行と記号を含む）。
pub const MAIL_BODY: &str = "1行目 & 記号 = ? #\n2行目: 日本語";
/// 固定の SMS 宛先（空。利用者が作成画面で選ぶ）。
pub const SMS_TO: &str = "";
/// 固定の SMS 本文。
pub const SMS_BODY: &str = "テスト & 確認\n2行目";

/// 送信待ちに入ったときの案内（送信完了は言わない）。
pub const QUEUED_STATUS: &str = "送信待ちに入りました（送信の完了は確認できません）";
/// 下書き保存のときの案内。
pub const SAVED_STATUS: &str = "下書きとして保存されました";
/// キャンセルされたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "作成をキャンセルしました";
/// OS に依頼しただけのときの案内。
pub const HANDED_OFF_STATUS: &str =
    "OS に作成画面を開くよう依頼しました（開いたか・送ったかは確認できません）";
/// 扱えるアプリが無いときの案内（失敗扱いにしない）。
pub const NO_HANDLER_STATUS: &str = "メール・SMS を扱えるアプリや設定が見つかりません";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境ではこの作成画面を開けません";
/// 入力不正の接頭辞。
pub const INVALID_PREFIX: &str = "入力を確認してください: ";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

const NOTE_TEXT: &str =
    "固定の宛先・件名・本文で OS の作成画面を開きます（送信は利用者が OS 側で行います）";
const MAIL_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const SMS_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 作成画面の種類（`fandhe_device::compose_mail_sms::ComposeKind` に対応。写像は proxy 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeTarget {
    /// メール。
    Mail,
    /// SMS。
    Sms,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComposePhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_composing`] 済み）。
    Composing,
}

/// OS が判定した結果（`ComposeOutcome` に非対応を加えたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 送信待ちに入った。
    Queued,
    /// 下書き保存。
    Saved,
    /// キャンセル。
    Cancelled,
    /// OS に依頼しただけ。
    HandedOff,
    /// 扱うアプリが無い。
    NoHandler,
    /// この環境では非対応。
    Unsupported,
}

/// 失敗（画面には固定文言だけを出す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeFailure {
    /// 入力不正（OS へは渡していない）。
    Invalid(String),
    /// 内部失敗。
    Failed(String),
}

/// effect ドライバへ渡す要求。値は未検査で、検査は proxy 側（`ComposeRequest`）が行う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeRequestView {
    /// [`finish_compose`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 種類。
    pub target: ComposeTarget,
    /// 宛先。
    pub to: String,
    /// 件名（SMS は空）。
    pub subject: String,
    /// 本文。
    pub body: String,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposeMailSmsState {
    /// 進行状態。
    pub phase: ComposePhase,
    /// 直近に押された種類。
    pub target: Option<ComposeTarget>,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 入力不正・失敗（`TextRole::Error`）。
    pub error: String,
}

fn request(state: &mut ComposeMailSmsState, target: ComposeTarget) {
    if state.phase != ComposePhase::Idle {
        return;
    }
    state.phase = ComposePhase::Requested;
    state.target = Some(target);
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// 「メールを作成」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn compose_mail(state: &mut ComposeMailSmsState) {
    request(state, ComposeTarget::Mail);
}

/// 「SMS を作成」ボタンのハンドラ。`Idle` 以外は no-op。
pub fn compose_sms(state: &mut ComposeMailSmsState) {
    request(state, ComposeTarget::Sms);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &ComposeMailSmsState) -> Option<ComposeRequestView> {
    if state.phase != ComposePhase::Requested {
        return None;
    }
    let target = state.target?;
    let (to, subject, body) = match target {
        ComposeTarget::Mail => (MAIL_TO, MAIL_SUBJECT, MAIL_BODY),
        ComposeTarget::Sms => (SMS_TO, "", SMS_BODY),
    };
    Some(ComposeRequestView {
        generation: state.generation,
        target,
        to: to.to_string(),
        subject: subject.to_string(),
        body: body.to_string(),
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_composing(state: &mut ComposeMailSmsState) {
    if state.phase == ComposePhase::Requested {
        state.phase = ComposePhase::Composing;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_compose(
    state: &mut ComposeMailSmsState,
    generation: u64,
    result: Result<CoreOutcome, ComposeFailure>,
) {
    if state.phase == ComposePhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ComposePhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::Queued) => state.status = QUEUED_STATUS.to_string(),
        Ok(CoreOutcome::Saved) => state.status = SAVED_STATUS.to_string(),
        Ok(CoreOutcome::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Ok(CoreOutcome::HandedOff) => state.status = HANDED_OFF_STATUS.to_string(),
        Ok(CoreOutcome::NoHandler) => state.status = NO_HANDLER_STATUS.to_string(),
        Ok(CoreOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(ComposeFailure::Invalid(m)) => state.error = format!("{INVALID_PREFIX}{m}"),
        Err(ComposeFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &ComposeMailSmsState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &ComposeMailSmsState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外し、同時に 2 件出さない。
pub fn current_screen(state: &ComposeMailSmsState) -> Screen<ComposeMailSmsState> {
    let screen = Screen::new("メール・SMS の作成")
        .text(NOTE_TEXT)
        .text(format!("メール宛先: {MAIL_TO}"))
        .text(format!("メール件名: {MAIL_SUBJECT}"))
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != ComposePhase::Idle {
        return screen;
    }
    screen
        .button(MAIL_BUTTON_ID, "メールを作成", compose_mail)
        .button(SMS_BUTTON_ID, "SMS を作成", compose_sms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composing(target: ComposeTarget) -> ComposeMailSmsState {
        let mut s = ComposeMailSmsState::default();
        request(&mut s, target);
        mark_composing(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, ComposeFailure>) -> (String, String) {
        let mut s = composing(ComposeTarget::Mail);
        let g = s.generation;
        finish_compose(&mut s, g, r);
        assert_eq!(s.phase, ComposePhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn eight_results_are_distinct_and_none_claims_sent() {
        let all = [
            shown(Ok(CoreOutcome::Queued)),
            shown(Ok(CoreOutcome::Saved)),
            shown(Ok(CoreOutcome::Cancelled)),
            shown(Ok(CoreOutcome::HandedOff)),
            shown(Ok(CoreOutcome::NoHandler)),
            shown(Ok(CoreOutcome::Unsupported)),
            shown(Err(ComposeFailure::Invalid("x".into()))),
            shown(Err(ComposeFailure::Failed("x".into()))),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
            let text = format!("{}{}", a.0, a.1);
            assert!(!text.contains("送信しました") && !text.contains("送信済み"));
        }
        assert_ne!(HANDED_OFF_STATUS, QUEUED_STATUS);
        assert_ne!(NO_HANDLER_STATUS, QUEUED_STATUS);
    }

    #[test]
    fn non_failures_do_not_go_to_error() {
        for o in [
            CoreOutcome::Cancelled,
            CoreOutcome::Saved,
            CoreOutcome::NoHandler,
        ] {
            let (status, error) = shown(Ok(o));
            assert!(!status.is_empty() && error.is_empty());
        }
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = composing(ComposeTarget::Sms);
        let g = s.generation + 1;
        finish_compose(&mut s, g, Ok(CoreOutcome::Queued));
        assert_eq!(s.phase, ComposePhase::Composing);
        assert!(s.status.is_empty());

        let mut idle = ComposeMailSmsState::default();
        finish_compose(&mut idle, 0, Ok(CoreOutcome::Queued));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn request_is_noop_while_in_flight_and_take_request_matches_target() {
        let mut s = ComposeMailSmsState::default();
        assert!(take_request(&s).is_none());
        compose_sms(&mut s);
        let r = take_request(&s).unwrap();
        assert_eq!(
            (r.target, r.to.as_str(), r.subject.as_str()),
            (ComposeTarget::Sms, SMS_TO, "")
        );
        compose_mail(&mut s);
        assert_eq!(s.target, Some(ComposeTarget::Sms));
        mark_composing(&mut s);
        assert!(take_request(&s).is_none());
        compose_mail(&mut s);
        assert_eq!(s.phase, ComposePhase::Composing);
    }

    #[test]
    fn buttons_hidden_while_busy() {
        let idle = current_screen(&ComposeMailSmsState::default());
        let busy = current_screen(&composing(ComposeTarget::Mail));
        assert!(format!("{idle:?}").contains("SMS を作成"));
        assert!(!format!("{busy:?}").contains("SMS を作成"));
    }
}
