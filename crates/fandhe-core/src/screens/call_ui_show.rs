//! Phase 11（端末機能の拡充ループ、親 #87）型サンプル「通話 UI 統合」（Issue #276）。
//! 「着信を模擬する」を押すと OS の着信 UI に疑似着信を 1 件出し、応答・拒否・終了の結果を表示する 1 画面。
//! `FANDHE_PROXY_APP=call_ui_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod call_ui_show;` と対）。範囲は疑似着信 1 件のみで、
//! 音声・発信・履歴・複数通話・電話番号の入力は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/call_ui.rs` が [`take_command`] で要求を観測し、[`apply_update`] で
//! 通知を注入する（`midi_show.rs` と同型）。iOS・Android の操作案内も含め、全対象 OS の状態・文言を
//! ここで確定済みで、後続の OS 実装（#277・#278）は本画面を触らない。
//!
//! ```text
//! Idle/Ended/Failed --「着信を模擬する」--> Reporting（generation += 1, command = Report）
//!   --Ringing--> Ringing --Answered--> Active
//! Ringing --「着信を取り消す」--> Ended(LocalEnded)（generation += 1, command = End）
//! Active  --「通話を終了する」--> Ended(LocalEnded)（同上）
//! Ringing/Active --Ended(理由)--> Ended(理由)   Reporting/Ringing/Active --Failed--> Failed
//! ```
//!
//! 世代の違う通知と、段階に合わない通知は捨てる。終了（拒否・取り消し・不応答を含む）は失敗として
//! 見せない（[`status_text`] は通常色、[`error_text`] は `Failed` だけ）。

use crate::{Screen, WidgetId};

const REPORT_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const END_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 画面タイトル。
pub const TITLE: &str = "通話 UI 統合";
/// 着信を出すボタンのラベル。
pub const REPORT_LABEL: &str = "着信を模擬する";
/// 呼び出し中に押す取り消しボタンのラベル。
pub const CANCEL_LABEL: &str = "着信を取り消す";
/// 通話中に押す終了ボタンのラベル。
pub const HANGUP_LABEL: &str = "通話を終了する";
/// 着信要求中の案内。
pub const REPORTING_STATUS: &str = "着信を出しています…";
/// 呼び出し中の案内。
pub const RINGING_STATUS: &str = "着信中です";
/// 通話中の案内。
pub const ACTIVE_STATUS: &str = "通話中です";
/// OS ごとの操作案内（iOS は OS の着信画面、Android は通知のボタン）。
pub const HINT: &str = "iOS は OS の着信画面、Android は通知の応答・拒否ボタンで操作します";
/// 拒否で終わったときの案内（失敗ではない）。
pub const DECLINED_STATUS: &str = "着信を拒否しました";
/// 相手側（OS）の終了の案内。
pub const REMOTE_ENDED_STATUS: &str = "通話が終了しました";
/// 画面から終了したときの案内。
pub const LOCAL_ENDED_STATUS: &str = "通話を終了しました";
/// 応答がなかったときの案内。
pub const UNANSWERED_STATUS: &str = "応答がありませんでした";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境では通話 UI を使えません";
/// 通知が許可されていないときの案内。
pub const PERMISSION_DENIED_STATUS: &str = "通知が許可されていません";
/// OS が着信を受け付けなかったときの案内。
pub const DISALLOWED_STATUS: &str = "OS が着信を受け付けませんでした";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 要求の期限切れの失敗理由。
pub const REPORT_TIMEOUT: &str = "着信を出せませんでした（期限切れ）";

/// 通話の終わり方（`fandhe_device::call_ui::EndReason` の写し。依存方向のため複製）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallEnd {
    /// 拒否。
    Declined,
    /// 相手側（OS）の終了。
    RemoteEnded,
    /// 画面からの取り消し・終了。
    LocalEnded,
    /// 不応答。
    Unanswered,
}

/// 失敗の種別（`CallUiFailure` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallFailure {
    /// 非対応。
    Unsupported,
    /// 通知が許可されていない。
    PermissionDenied,
    /// OS が着信を拒んだ。
    Disallowed,
    /// それ以外（固定文言のみ。OS の生コードを入れない）。
    Failed(String),
}

/// 画面側の進行状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CallPhase {
    /// 未開始。
    #[default]
    Idle,
    /// 着信要求中（受理待ち）。
    Reporting,
    /// 呼び出し中。
    Ringing,
    /// 通話中。
    Active,
    /// 終了（失敗ではない）。
    Ended(CallEnd),
    /// 失敗。
    Failed(CallFailure),
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallCommand {
    /// 着信を出す。
    Report,
    /// 取り消し・切断する。
    End,
}

/// effect ドライバから注入する通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallUpdate {
    /// OS が着信を受理した。
    Ringing,
    /// 応答された。
    Answered,
    /// 終了した。
    Ended(CallEnd),
    /// 失敗した。
    Failed(CallFailure),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CallUiShowState {
    /// 進行状態。
    pub phase: CallPhase,
    /// 直近に立てた要求の世代。
    pub generation: u64,
    /// 未発送の要求。
    pub command: Option<CallCommand>,
}

/// 「着信を模擬する」。`Idle` / `Ended` / `Failed` のときだけ受け付ける。
pub fn report(state: &mut CallUiShowState) {
    if !matches!(
        state.phase,
        CallPhase::Idle | CallPhase::Ended(_) | CallPhase::Failed(_)
    ) {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.phase = CallPhase::Reporting;
    state.command = Some(CallCommand::Report);
}

/// 「着信を取り消す」（`Ringing`）と「通話を終了する」（`Active`）の共用。
pub fn end_call(state: &mut CallUiShowState) {
    if !matches!(state.phase, CallPhase::Ringing | CallPhase::Active) {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.phase = CallPhase::Ended(CallEnd::LocalEnded);
    state.command = Some(CallCommand::End);
}

/// 未発送の要求と世代を観測する getter（状態は変えない）。
pub fn take_command(state: &CallUiShowState) -> Option<(CallCommand, u64)> {
    state.command.map(|c| (c, state.generation))
}

/// 要求を発送済みにする。世代が違えば何もしない。
pub fn mark_command_taken(state: &mut CallUiShowState, generation: u64) {
    if state.generation == generation {
        state.command = None;
    }
}

/// 通知を反映する。世代違いと、段階に合わない通知は捨てる。
pub fn apply_update(state: &mut CallUiShowState, generation: u64, update: CallUpdate) {
    if state.generation != generation {
        return;
    }
    match (&state.phase, update) {
        (CallPhase::Reporting, CallUpdate::Ringing) => state.phase = CallPhase::Ringing,
        (CallPhase::Ringing, CallUpdate::Answered) => state.phase = CallPhase::Active,
        (CallPhase::Ringing | CallPhase::Active, CallUpdate::Ended(e)) => {
            state.phase = CallPhase::Ended(e)
        }
        (CallPhase::Reporting | CallPhase::Ringing | CallPhase::Active, CallUpdate::Failed(f)) => {
            state.phase = CallPhase::Failed(f)
        }
        _ => {}
    }
}

/// 案内文言（通常色）。終了の文言もここに出す。
pub fn status_text(state: &CallUiShowState) -> String {
    match &state.phase {
        CallPhase::Idle | CallPhase::Failed(_) => String::new(),
        CallPhase::Reporting => REPORTING_STATUS.to_string(),
        CallPhase::Ringing => RINGING_STATUS.to_string(),
        CallPhase::Active => ACTIVE_STATUS.to_string(),
        CallPhase::Ended(CallEnd::Declined) => DECLINED_STATUS.to_string(),
        CallPhase::Ended(CallEnd::RemoteEnded) => REMOTE_ENDED_STATUS.to_string(),
        CallPhase::Ended(CallEnd::LocalEnded) => LOCAL_ENDED_STATUS.to_string(),
        CallPhase::Ended(CallEnd::Unanswered) => UNANSWERED_STATUS.to_string(),
    }
}

/// 失敗の案内（`TextRole::Error`）。失敗以外は空。
pub fn error_text(state: &CallUiShowState) -> String {
    match &state.phase {
        CallPhase::Failed(CallFailure::Unsupported) => UNSUPPORTED_STATUS.to_string(),
        CallPhase::Failed(CallFailure::PermissionDenied) => PERMISSION_DENIED_STATUS.to_string(),
        CallPhase::Failed(CallFailure::Disallowed) => DISALLOWED_STATUS.to_string(),
        CallPhase::Failed(CallFailure::Failed(m)) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 画面を組み立てる。`Reporting` 中はボタンを出さない。
pub fn current_screen(state: &CallUiShowState) -> Screen<CallUiShowState> {
    let screen = Screen::new(TITLE)
        .text(HINT)
        .dynamic_text(status_text)
        .error_text(error_text);
    match state.phase {
        CallPhase::Idle | CallPhase::Ended(_) | CallPhase::Failed(_) => {
            screen.button(REPORT_BUTTON_ID, REPORT_LABEL, report)
        }
        CallPhase::Ringing => screen.button(END_BUTTON_ID, CANCEL_LABEL, end_call),
        CallPhase::Active => screen.button(END_BUTTON_ID, HANGUP_LABEL, end_call),
        CallPhase::Reporting => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(s: &mut CallUiShowState, u: CallUpdate) {
        let g = s.generation;
        apply_update(s, g, u);
    }

    fn ringing() -> CallUiShowState {
        let mut s = CallUiShowState::default();
        report(&mut s);
        upd(&mut s, CallUpdate::Ringing);
        s
    }

    #[test]
    fn default_requests_nothing_and_end_is_invalid_in_idle() {
        let mut s = CallUiShowState::default();
        assert_eq!(take_command(&s), None);
        end_call(&mut s);
        assert_eq!(s, CallUiShowState::default());
    }

    #[test]
    fn report_is_not_doubled_while_reporting() {
        let mut s = CallUiShowState::default();
        report(&mut s);
        assert_eq!(take_command(&s), Some((CallCommand::Report, 1)));
        mark_command_taken(&mut s, 0);
        assert!(take_command(&s).is_some(), "世代違いでは消えない");
        mark_command_taken(&mut s, 1);
        assert_eq!(take_command(&s), None);
        report(&mut s);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = ringing();
        let g = s.generation;
        apply_update(&mut s, g - 1, CallUpdate::Answered);
        assert_eq!(s.phase, CallPhase::Ringing);
    }

    #[test]
    fn mismatched_updates_are_dropped() {
        let mut s = CallUiShowState::default();
        upd(&mut s, CallUpdate::Ringing);
        assert_eq!(s.phase, CallPhase::Idle);
        report(&mut s);
        upd(&mut s, CallUpdate::Answered);
        assert_eq!(s.phase, CallPhase::Reporting);
        upd(&mut s, CallUpdate::Ended(CallEnd::Declined));
        assert_eq!(s.phase, CallPhase::Reporting);
        let mut e = ringing();
        upd(&mut e, CallUpdate::Ended(CallEnd::Declined));
        upd(&mut e, CallUpdate::Answered);
        assert_eq!(e.phase, CallPhase::Ended(CallEnd::Declined));
    }

    #[test]
    fn late_answer_after_cancel_is_not_active() {
        let mut s = ringing();
        let old = s.generation;
        end_call(&mut s);
        assert_eq!(take_command(&s).map(|c| c.0), Some(CallCommand::End));
        apply_update(&mut s, old, CallUpdate::Answered);
        assert_eq!(s.phase, CallPhase::Ended(CallEnd::LocalEnded));
    }

    #[test]
    fn normal_ends_are_not_errors() {
        for e in [
            CallEnd::Declined,
            CallEnd::RemoteEnded,
            CallEnd::LocalEnded,
            CallEnd::Unanswered,
        ] {
            let mut s = ringing();
            upd(&mut s, CallUpdate::Ended(e));
            assert_eq!(error_text(&s), "");
            assert!(!status_text(&s).is_empty());
        }
    }

    #[test]
    fn failure_messages_differ_and_are_non_empty() {
        let mut texts = Vec::new();
        for f in [
            CallFailure::Unsupported,
            CallFailure::PermissionDenied,
            CallFailure::Disallowed,
            CallFailure::Failed(REPORT_TIMEOUT.into()),
        ] {
            let mut s = CallUiShowState::default();
            report(&mut s);
            upd(&mut s, CallUpdate::Failed(f));
            let t = error_text(&s);
            assert!(!t.is_empty());
            assert_eq!(status_text(&s), "");
            texts.push(t);
        }
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), 4);
    }

    #[test]
    fn can_report_again_after_end_or_failure() {
        let mut s = ringing();
        upd(&mut s, CallUpdate::Ended(CallEnd::Unanswered));
        report(&mut s);
        assert_eq!(s.phase, CallPhase::Reporting);
        upd(&mut s, CallUpdate::Failed(CallFailure::Disallowed));
        report(&mut s);
        assert_eq!(s.phase, CallPhase::Reporting);
    }
}
