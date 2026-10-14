//! Phase 11 の拡充ループ（親 #1116）型サンプル「カスタム URL scheme」（Issue #1215）。
//! `FANDHE_PROXY_APP=url_scheme_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod url_scheme_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS から届いた URL の検査・保持は
//! `fandhe_device::url_scheme`、スナップショットを本モジュールへ注入するのは
//! `fandhe-egui-proxy` の effect ドライバ（`device/url_scheme.rs`）の責務。
//!
//! Issue #1216（Windows）: Windows にはアプリ側の宣言で scheme を登録する手段がないため、
//! 画面の操作でレジストリ登録・解除を行う（`registrable` が立った OS だけボタンを出す。
//! 要求の受け渡しは `background_task` と同型の `Phase` / 世代番号）。
//! 登録 UI の状態は 1 系統で、Windows と Web（`fandhe-web-dom`）が共用する。
//!
//! 「使えない」と「まだ届いていない」は別の状態（[`UrlSchemeView::Unsupported`] と
//! [`UrlSchemeView::Waiting`]）で、文言も分ける。弾いた URL は中身を表示せず件数だけ出す。

use crate::screens::background_task::format_utc;
use crate::{Screen, WidgetId};

/// 受信状態の表示区分。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum UrlSchemeView {
    /// まだ読み取っていない（effect ドライバが最初の結果を注入するまで）。
    #[default]
    Unread,
    /// この OS では URL scheme を受信できない（失敗ではない）。
    Unsupported,
    /// 受信できる状態だが、まだ URL を受理していない。
    Waiting,
    /// 最後に受理した URL と受信時刻（UNIX ms）。
    Received {
        /// 検査済みの URL 文字列。
        url: String,
        /// 受信時刻。
        received_at_unix_ms: u64,
    },
}

/// 登録・解除の操作種別（ボタンに対応）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 「この scheme を登録する」。
    Register,
    /// 「登録を解除する」。
    Unregister,
}

/// ボタン操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested(Action),
    /// effect ドライバが OS 呼び出しを処理中。
    InFlight,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// [`finish_action`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 実行する操作。
    pub action: Action,
}

/// 「カスタム URL scheme」画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UrlSchemeShowState {
    view: UrlSchemeView,
    rejected: u64,
    seq: Option<u64>,
    registrable: bool,
    phase: Phase,
    generation: u64,
    status: String,
    error: String,
}

/// 操作が成功したときの種類。Web は確定を検知できないため区別する（成功を装わない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionOutcome {
    /// 登録・解除が完了した（Windows のレジストリ操作）。
    Done,
    /// 登録を要求しただけで、確定は検知できない（Web の `registerProtocolHandler`）。
    AwaitingConfirmation,
}

/// 操作の失敗種別。「使えない」と「失敗」を分ける（成功を装わない）。
#[derive(Debug, Clone, PartialEq)]
pub enum ActionFailure {
    /// 環境がその操作を持たない。
    Unsupported,
    /// 試みたが失敗した（利用者向けの完成した文言。そのまま表示する）。
    Failed(String),
}

/// 登録ボタンの widget id（Windows は [`current_screen`]、Web はエントリポイントが使う）。
pub const REGISTER_BUTTON_ID: crate::WidgetId = crate::WidgetId::from_raw(1);

impl UrlSchemeShowState {
    /// 最後に適用したスナップショットの世代番号（effect ドライバが変化検出に使う）。
    /// 未適用なら `None`。
    pub fn seq(&self) -> Option<u64> {
        self.seq
    }

    /// 画面からの登録・解除が要る OS として確定済みか（effect ドライバが一度だけ立てるために使う）。
    pub fn registrable(&self) -> bool {
        self.registrable
    }

    /// 非対応として確定済みか（effect ドライバが一度だけ送るために使う）。
    pub fn is_unsupported(&self) -> bool {
        self.view == UrlSchemeView::Unsupported
    }
}

/// この OS が画面からの登録・解除を要するか（effect ドライバが一度だけ立てる）。
pub fn set_registrable(state: &mut UrlSchemeShowState) {
    state.registrable = true;
}

fn request(state: &mut UrlSchemeShowState, action: Action) {
    if state.phase != Phase::Idle {
        return;
    }
    state.phase = Phase::Requested(action);
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// 「登録する」ボタンのハンドラ。処理中は no-op（二重要求の防止）。
pub fn register(state: &mut UrlSchemeShowState) {
    request(state, Action::Register);
}

/// 「登録を解除する」ボタンのハンドラ。処理中は no-op。
pub fn unregister(state: &mut UrlSchemeShowState) {
    request(state, Action::Unregister);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &UrlSchemeShowState) -> Option<Request> {
    match state.phase {
        Phase::Requested(action) => Some(Request {
            generation: state.generation,
            action,
        }),
        _ => None,
    }
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut UrlSchemeShowState) {
    if matches!(state.phase, Phase::Requested(_)) {
        state.phase = Phase::InFlight;
    }
}

/// 操作結果を反映する。世代が違う、または要求が無いときの完了は無視する。
/// 失敗は `error` にだけ入れ、成功文言を出さない。Web と Windows で共用する。
pub fn finish_action(
    state: &mut UrlSchemeShowState,
    generation: u64,
    action: Action,
    result: Result<ActionOutcome, ActionFailure>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    match result {
        Ok(ActionOutcome::Done) => {
            state.status = match action {
                Action::Register => "登録しました".to_string(),
                Action::Unregister => "登録を解除しました".to_string(),
            };
        }
        Ok(ActionOutcome::AwaitingConfirmation) => {
            state.status =
                "登録を要求しました（ブラウザの確認で許可されると有効になります）".to_string();
        }
        Err(ActionFailure::Unsupported) => {
            state.error = "このブラウザでは登録できません".to_string();
        }
        Err(ActionFailure::Failed(message)) => state.error = message,
    }
}

/// 登録・解除の結果文言（未操作なら空）。
pub fn registration_text(state: &UrlSchemeShowState) -> String {
    state.status.clone()
}

/// 登録・解除の失敗文言（`TextRole::Error` で描画される）。
pub fn error_text(state: &UrlSchemeShowState) -> String {
    state.error.clone()
}

/// 受信状態のスナップショットを反映する。`last` は `(url, 受信時刻 UNIX ms)`。
pub fn apply_snapshot(
    state: &mut UrlSchemeShowState,
    seq: u64,
    last: Option<(String, u64)>,
    rejected: u64,
) {
    state.view = match last {
        Some((url, received_at_unix_ms)) => UrlSchemeView::Received {
            url,
            received_at_unix_ms,
        },
        None => UrlSchemeView::Waiting,
    };
    state.rejected = rejected;
    state.seq = Some(seq);
}

/// この OS では受信できないことを反映する。
pub fn apply_unsupported(state: &mut UrlSchemeShowState) {
    state.view = UrlSchemeView::Unsupported;
}

/// 状態行の文言。
pub fn status_text(state: &UrlSchemeShowState) -> String {
    match &state.view {
        UrlSchemeView::Unread => "読み取り中".to_string(),
        UrlSchemeView::Unsupported => "この OS では未対応です".to_string(),
        UrlSchemeView::Waiting => {
            "まだ受け取っていません（fandhe:// / web+fandhe: の URL を開くと届きます）".to_string()
        }
        UrlSchemeView::Received { .. } => "受け取りました".to_string(),
    }
}

/// 最後に受け取った URL（未受信なら空）。
pub fn url_text(state: &UrlSchemeShowState) -> String {
    match &state.view {
        UrlSchemeView::Received { url, .. } => format!("URL: {url}"),
        _ => String::new(),
    }
}

/// 受信時刻（未受信なら空）。
pub fn time_text(state: &UrlSchemeShowState) -> String {
    match &state.view {
        UrlSchemeView::Received {
            received_at_unix_ms,
            ..
        } => format!("受信時刻: {}", format_utc(*received_at_unix_ms)),
        _ => String::new(),
    }
}

/// 検査で弾いた件数の文言。
pub fn rejected_text(state: &UrlSchemeShowState) -> String {
    format!("検査で弾いた件数: {}", state.rejected)
}

/// 「登録を解除する」ボタンの id。
const UNREGISTER_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 「カスタム URL scheme」画面。
pub fn url_scheme_show_screen() -> Screen<UrlSchemeShowState> {
    Screen::new("カスタム URL scheme")
        .dynamic_text(status_text)
        .dynamic_text(url_text)
        .dynamic_text(time_text)
        .dynamic_text(rejected_text)
        .dynamic_text(registration_text)
        .error_text(error_text)
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
/// 登録ボタンは登録が要る OS（`registrable`）で、処理中でないときだけ出す。
pub fn current_screen(state: &UrlSchemeShowState) -> Screen<UrlSchemeShowState> {
    let screen = url_scheme_show_screen();
    if !state.registrable || state.phase != Phase::Idle {
        return screen;
    }
    screen
        .button(REGISTER_BUTTON_ID, "この scheme を登録する", register)
        .button(UNREGISTER_BUTTON_ID, "登録を解除する", unregister)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    #[test]
    fn unsupported_and_waiting_have_different_text() {
        let mut unsupported = UrlSchemeShowState::default();
        apply_unsupported(&mut unsupported);
        let mut waiting = UrlSchemeShowState::default();
        apply_snapshot(&mut waiting, 0, None, 0);
        assert_ne!(status_text(&unsupported), status_text(&waiting));
        assert!(unsupported.is_unsupported());
        assert_eq!(waiting.seq(), Some(0));
    }

    #[test]
    fn received_shows_url_time_and_rejected() {
        let mut state = UrlSchemeShowState::default();
        apply_snapshot(&mut state, 3, Some(("fandhe://hello?x=1".into(), 0)), 2);
        let joined = render_headless(&url_scheme_show_screen(), &state).join("\n");
        assert!(joined.contains("fandhe://hello?x=1"));
        assert!(joined.contains("1970-01-01 00:00:00 UTC"));
        assert!(joined.contains("検査で弾いた件数: 2"));
    }

    #[test]
    fn register_flow_is_guarded_and_never_claims_done() {
        let mut s = UrlSchemeShowState::default();
        finish_action(
            &mut s,
            0,
            Action::Register,
            Ok(ActionOutcome::AwaitingConfirmation),
        );
        assert_eq!(registration_text(&s), "");
        assert!(take_request(&s).is_none());
        register(&mut s);
        let req = take_request(&s).expect("要求がある");
        finish_action(
            &mut s,
            req.generation,
            req.action,
            Ok(ActionOutcome::AwaitingConfirmation),
        );
        assert!(take_request(&s).is_none());
        assert!(registration_text(&s).contains("要求"));
        assert!(!registration_text(&s).contains("登録しました"));
        register(&mut s);
        let req = take_request(&s).expect("要求がある");
        finish_action(
            &mut s,
            req.generation,
            req.action,
            Err(ActionFailure::Unsupported),
        );
        assert!(error_text(&s).contains("できません"));
        assert_eq!(registration_text(&s), "");
        register(&mut s);
        let req = take_request(&s).expect("要求がある");
        finish_action(
            &mut s,
            req.generation,
            req.action,
            Err(ActionFailure::Failed("x".into())),
        );
        assert_eq!(error_text(&s), "x");
    }

    #[test]
    fn done_text_follows_action() {
        let mut s = UrlSchemeShowState::default();
        register(&mut s);
        let req = take_request(&s).expect("要求がある");
        finish_action(
            &mut s,
            req.generation,
            Action::Register,
            Ok(ActionOutcome::Done),
        );
        assert_eq!(registration_text(&s), "登録しました");
        unregister(&mut s);
        let req = take_request(&s).expect("要求がある");
        finish_action(
            &mut s,
            req.generation,
            Action::Unregister,
            Ok(ActionOutcome::Done),
        );
        assert_eq!(registration_text(&s), "登録を解除しました");
    }

    #[test]
    fn waiting_hides_url_and_time() {
        let mut state = UrlSchemeShowState::default();
        apply_snapshot(&mut state, 1, None, 1);
        assert_eq!(url_text(&state), "");
        assert_eq!(time_text(&state), "");
        assert_eq!(rejected_text(&state), "検査で弾いた件数: 1");
    }

    #[test]
    fn buttons_only_when_registrable_and_idle() {
        let mut state = UrlSchemeShowState::default();
        let ids = |s: &UrlSchemeShowState| render_headless(&current_screen(s), s).join("\n");
        assert!(!ids(&state).contains("登録する"));
        set_registrable(&mut state);
        assert!(ids(&state).contains("この scheme を登録する"));
        register(&mut state);
        assert_eq!(state.phase, Phase::Requested(Action::Register));
        assert!(!ids(&state).contains("登録する"));
        mark_in_flight(&mut state);
        assert!(!ids(&state).contains("登録する"));
    }

    #[test]
    fn request_flow_ignores_stale_generation_and_keeps_failure_out_of_status() {
        let mut state = UrlSchemeShowState::default();
        register(&mut state);
        let req = take_request(&state).expect("要求がある");
        mark_in_flight(&mut state);
        assert_eq!(take_request(&state), None);
        finish_action(
            &mut state,
            req.generation + 1,
            req.action,
            Ok(ActionOutcome::Done),
        );
        assert_eq!(state.phase, Phase::InFlight);
        finish_action(
            &mut state,
            req.generation,
            req.action,
            Err(ActionFailure::Failed("x".into())),
        );
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(registration_text(&state), "");
        assert_eq!(error_text(&state), "x");
        // 完了後の重複通知は無視する。
        finish_action(
            &mut state,
            req.generation,
            req.action,
            Ok(ActionOutcome::Done),
        );
        assert_eq!(registration_text(&state), "");
    }

    #[test]
    fn waiting_text_is_os_neutral() {
        let mut state = UrlSchemeShowState::default();
        apply_snapshot(&mut state, 0, None, 0);
        assert!(!status_text(&state).contains("open"));
    }
}
