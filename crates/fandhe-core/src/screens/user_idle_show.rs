//! Phase 11 の拡充ループ（親 #125）型サンプル「無操作時間の検知」（Issue #631）。
//! `FANDHE_PROXY_APP=user_idle_show` で選ばれ、`fandhe-egui-proxy` の `screens/registry.rs` の
//! `effect` 区画（native）と `fandhe-web-dom`（Web、#632）から呼ばれる。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。native は
//! `fandhe-egui-proxy` の `device/user_idle.rs` が 1 秒間隔で最後の入力からの経過秒を読み、
//! [`apply_read`] で注入する。Web は経過秒が取れず（Idle Detection API は操作中 / 離席中と画面ロック
//! の状態だけを返す）、許可ダイアログを要するためボタン起点（[`request_watch`] /
//! [`pending_request`] / [`finish_request`]）で、世代を照合して古い完了を捨てる。pagehide /
//! bfcache 復帰は [`pause_web`] / [`resume_web`] で扱う。
//!
//! 全対象 OS の状態・定数・文言をここで確定済みで、後続の Web 実装（#632）は本画面を触らない。
//!
//! 嘘の「操作中」を出さない: 未読み取り・失敗・非対応・許可なし・一時停止では、前回の値を残さず
//! 「判定できません」と出す。しきい値は [`IDLE_THRESHOLD`] 以上（`>=`）で「離席中」とする
//! （Web の IdleDetector がしきい値に達した時点で idle になる動きにそろえる）。

use std::time::Duration;

use crate::{Screen, WidgetId};

/// 離席中とみなすしきい値の SSOT。Web の `start({threshold})` の下限（60,000 ms）にそろえ、
/// #632 はこの値の ms を渡す。
pub const IDLE_THRESHOLD: Duration = Duration::from_secs(60);
/// 題名。
pub const TITLE: &str = "無操作時間の検知";
/// 値が取れない項目の表示。
pub const UNKNOWN_TEXT: &str = "取得できません";
/// 判定できないときの在席表示（「操作中」にしない）。
pub const UNDETERMINED_TEXT: &str = "判定できません";
/// 失敗時の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// Web の許可ボタンの ID。
const REQUEST_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// Web の許可ボタンの文言。
pub const REQUEST_LABEL: &str = "離席の検知を始める（許可を求めます）";
/// Web で経過秒を出さない旨（数えた値は出さない）。
pub const WEB_ELAPSED_TEXT: &str = "このブラウザでは経過秒を取得できません（状態だけを表示します）";
/// native で画面ロックを出さない旨。
pub const NATIVE_LOCK_TEXT: &str = "この OS では表示しません";
/// 反映遅れ・入力の定義の差・Web の対応範囲の注記。
pub const NOTE: &str = "反映は最大 1 秒遅れます。「入力」の扱い（合成入力・タブレット・ゲームパッド）は OS によって異なります。Web は Chromium 系のブラウザだけで使えます。";

/// 在席状態の表示用（device / Web の `userState` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserPresenceView {
    /// 操作中。
    Active,
    /// 離席中。
    Idle,
}

/// 画面ロックの表示用（Web の `screenState` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLockView {
    /// ロック中。
    Locked,
    /// ロックされていない。
    Unlocked,
}

/// Web の 1 回分の読み取り（状態だけ。経過秒は無い）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebIdleReading {
    /// 在席状態。
    pub user: UserPresenceView,
    /// 画面ロック。
    pub screen: ScreenLockView,
}

/// 読めなかった理由。`DeviceError` / ブラウザのエラーからの写像は呼び出し元の責務。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// この環境では読めない。
    Unsupported,
    /// 利用者が許可しなかった（Web）。失敗とは別扱い。
    PermissionDenied,
    /// 内部エラー（固定文言のみ。OS 固有値を持たない）。
    Failed(String),
}

/// 画面に出す状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IdleView {
    /// native の初期。最初の値が届く前。
    #[default]
    Reading,
    /// native の経過秒。
    Elapsed(u64),
    /// Web の初期。ボタン待ち。
    AwaitingRequest,
    /// Web の許可ダイアログ待ち（許可 + 開始）。
    Requesting,
    /// Web の bfcache 復帰後の再開待ち（許可済みのため開始だけ）。
    Resuming,
    /// Web の一時停止（pagehide 後）。
    Paused,
    /// Web の監視中。
    Watching(WebIdleReading),
    /// 許可なし（Web）。
    PermissionDenied,
    /// 非対応。
    Unsupported,
    /// 失敗。
    Failed(String),
}

/// 「無操作時間の検知」画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserIdleShowState {
    /// 現在の表示状態。
    pub view: IdleView,
    /// Web の要求の世代。
    pub generation: u64,
    /// Web か（経過秒・画面ロックの文言が変わる）。
    pub is_web: bool,
}

/// Web の要求の種別（#632 が `pending_request` で観測する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRequestKind {
    /// `requestPermission()` + `start()`。
    Permission,
    /// `start()` のみ（bfcache 復帰後）。
    Resume,
}

/// Web の初期状態（ボタン待ち）。
pub fn web_initial_state() -> UserIdleShowState {
    UserIdleShowState {
        view: IdleView::AwaitingRequest,
        generation: 0,
        is_web: true,
    }
}

/// native の読み取り結果を反映する。毎回 view を置き換えるため、失敗時に前回の経過秒は残らない。
pub fn apply_read(state: &mut UserIdleShowState, result: Result<Duration, ReadFailure>) {
    state.view = match result {
        Ok(d) => IdleView::Elapsed(d.as_secs()),
        Err(ReadFailure::Unsupported) => IdleView::Unsupported,
        Err(ReadFailure::PermissionDenied) => IdleView::PermissionDenied,
        Err(ReadFailure::Failed(m)) => IdleView::Failed(m),
    };
}

/// 現在の経過秒（effect ドライバの「前回と同じなら送らない」判定用）。
pub fn elapsed_secs(state: &UserIdleShowState) -> Option<u64> {
    match state.view {
        IdleView::Elapsed(s) => Some(s),
        _ => None,
    }
}

/// Web のボタンのハンドラ。`Requesting` 中は何もせず、それ以外は世代を進めて `Requesting` にする。
pub fn request_watch(state: &mut UserIdleShowState) {
    if state.view == IdleView::Requesting {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.view = IdleView::Requesting;
}

/// 未処理の要求があれば世代と種別を返す（Web の effect が観測する）。
pub fn pending_request(state: &UserIdleShowState) -> Option<(u64, WebRequestKind)> {
    match state.view {
        IdleView::Requesting => Some((state.generation, WebRequestKind::Permission)),
        IdleView::Resuming => Some((state.generation, WebRequestKind::Resume)),
        _ => None,
    }
}

/// 世代が現在のもので、結果を受け付ける view か。
fn accepts_web_result(state: &UserIdleShowState, generation: u64) -> bool {
    state.generation == generation
        && matches!(
            state.view,
            IdleView::Requesting | IdleView::Resuming | IdleView::Watching(_)
        )
}

/// Web の `start()` 完了を反映する。世代が一致し受付可能な view のときだけ反映する。
pub fn finish_request(
    state: &mut UserIdleShowState,
    generation: u64,
    result: Result<WebIdleReading, ReadFailure>,
) {
    if !accepts_web_result(state, generation) {
        return;
    }
    state.view = match result {
        Ok(r) => IdleView::Watching(r),
        Err(ReadFailure::Unsupported) => IdleView::Unsupported,
        Err(ReadFailure::PermissionDenied) => IdleView::PermissionDenied,
        Err(ReadFailure::Failed(m)) => IdleView::Failed(m),
    };
}

/// Web の `change` イベントを反映する。`start()` の解決より先に届いても受け入れる。
pub fn apply_web_change(state: &mut UserIdleShowState, generation: u64, reading: WebIdleReading) {
    if !accepts_web_result(state, generation) {
        return;
    }
    state.view = IdleView::Watching(reading);
}

/// pagehide・マウント破棄用。世代を進めて古い完了を無効にし、古い値を残さない。
pub fn pause_web(state: &mut UserIdleShowState) {
    state.generation = state.generation.wrapping_add(1);
    state.view = match state.view {
        IdleView::Watching(_) | IdleView::Resuming | IdleView::Paused => IdleView::Paused,
        IdleView::Requesting => IdleView::AwaitingRequest,
        ref other => other.clone(),
    };
}

/// pageshow（persisted）用。`Paused` のときだけ世代を進めて `Resuming` にする。
pub fn resume_web(state: &mut UserIdleShowState) {
    if state.view != IdleView::Paused {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.view = IdleView::Resuming;
}

/// 状態の文言。
pub fn status_text(state: &UserIdleShowState) -> String {
    match &state.view {
        IdleView::Reading => "読み取り中",
        IdleView::Elapsed(_) => "1 秒ごとに読み直しています",
        IdleView::AwaitingRequest => "ボタンを押すと、ブラウザが離席検知の許可を求めます",
        IdleView::Requesting => "許可を待っています",
        IdleView::Resuming => "検知を再開しています",
        IdleView::Paused => "一時停止中です",
        IdleView::Watching(_) => "検知中です",
        IdleView::PermissionDenied => "許可されませんでした。もう一度押すと再度許可を求めます",
        IdleView::Unsupported => "この環境では無操作時間を取得できません",
        IdleView::Failed(_) => "",
    }
    .to_string()
}

/// 経過秒の行。Web は経過秒が取れないので固定文言で、数えた値は出さない。
pub fn elapsed_text(state: &UserIdleShowState) -> String {
    if state.is_web {
        return WEB_ELAPSED_TEXT.to_string();
    }
    match state.view {
        IdleView::Elapsed(s) => format!("最後の操作から {s} 秒"),
        _ => UNKNOWN_TEXT.to_string(),
    }
}

/// 在席状態の行。成功時だけ「操作中」「離席中」を出し、それ以外は「判定できません」にする。
pub fn presence_text(state: &UserIdleShowState) -> String {
    let presence = match &state.view {
        IdleView::Elapsed(s) => Some(if *s >= IDLE_THRESHOLD.as_secs() {
            UserPresenceView::Idle
        } else {
            UserPresenceView::Active
        }),
        IdleView::Watching(r) => Some(r.user),
        _ => None,
    };
    let value = match presence {
        Some(UserPresenceView::Active) => "操作中".to_string(),
        Some(UserPresenceView::Idle) => {
            format!("離席中（{} 秒以上操作なし）", IDLE_THRESHOLD.as_secs())
        }
        None => UNDETERMINED_TEXT.to_string(),
    };
    format!("在席: {value}")
}

/// 画面ロックの行。Web の `Watching` だけ値を出す。
pub fn screen_lock_text(state: &UserIdleShowState) -> String {
    let value = if !state.is_web {
        NATIVE_LOCK_TEXT
    } else {
        match state.view {
            IdleView::Watching(r) => match r.screen {
                ScreenLockView::Locked => "ロック中",
                ScreenLockView::Unlocked => "ロックされていません",
            },
            _ => UNKNOWN_TEXT,
        }
    };
    format!("画面ロック: {value}")
}

/// 失敗時のみ非空。
pub fn error_text(state: &UserIdleShowState) -> String {
    match &state.view {
        IdleView::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 「無操作時間の検知」画面。許可ボタンは Web の操作が要る状態だけ付く。
pub fn current_screen(state: &UserIdleShowState) -> Screen<UserIdleShowState> {
    let screen = Screen::new(TITLE)
        .dynamic_text(status_text)
        .dynamic_text(elapsed_text)
        .dynamic_text(presence_text)
        .dynamic_text(screen_lock_text)
        .error_text(error_text)
        .text(NOTE);
    match state.view {
        IdleView::AwaitingRequest | IdleView::Requesting | IdleView::PermissionDenied => {
            screen.button(REQUEST_BUTTON_ID, REQUEST_LABEL, request_watch)
        }
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{click_button, render_headless};

    fn secs(s: u64) -> Result<Duration, ReadFailure> {
        Ok(Duration::from_secs(s))
    }

    fn reading(user: UserPresenceView, screen: ScreenLockView) -> WebIdleReading {
        WebIdleReading { user, screen }
    }

    fn active() -> WebIdleReading {
        reading(UserPresenceView::Active, ScreenLockView::Unlocked)
    }

    #[test]
    fn threshold_is_at_least_web_minimum() {
        assert!(IDLE_THRESHOLD >= Duration::from_millis(60_000));
    }

    #[test]
    fn presence_switches_at_threshold() {
        let mut s = UserIdleShowState::default();
        apply_read(&mut s, secs(59));
        assert_eq!(presence_text(&s), "在席: 操作中");
        apply_read(&mut s, secs(60));
        assert_eq!(presence_text(&s), "在席: 離席中（60 秒以上操作なし）");
        assert_eq!(elapsed_text(&s), "最後の操作から 60 秒");
    }

    #[test]
    fn non_success_is_never_active() {
        let mut views = vec![
            IdleView::Reading,
            IdleView::AwaitingRequest,
            IdleView::Requesting,
            IdleView::Resuming,
            IdleView::Paused,
            IdleView::PermissionDenied,
            IdleView::Unsupported,
            IdleView::Failed("x".into()),
        ];
        for v in views.drain(..) {
            let s = UserIdleShowState {
                view: v,
                ..Default::default()
            };
            assert_eq!(presence_text(&s), format!("在席: {UNDETERMINED_TEXT}"));
        }
    }

    #[test]
    fn failure_after_elapsed_drops_previous_value() {
        for f in [
            ReadFailure::Unsupported,
            ReadFailure::PermissionDenied,
            ReadFailure::Failed("無操作時間の取得".into()),
        ] {
            let mut s = UserIdleShowState::default();
            apply_read(&mut s, secs(30));
            apply_read(&mut s, Err(f));
            assert_eq!(elapsed_secs(&s), None);
            assert_eq!(elapsed_text(&s), UNKNOWN_TEXT);
        }
        let mut s = UserIdleShowState::default();
        apply_read(&mut s, Err(ReadFailure::Failed("無操作時間の取得".into())));
        assert_eq!(error_text(&s), "失敗しました: 無操作時間の取得");
    }

    #[test]
    fn web_elapsed_text_has_no_number() {
        let mut s = web_initial_state();
        request_watch(&mut s);
        finish_request(&mut s, 1, Ok(active()));
        assert!(!elapsed_text(&s).chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn screen_lock_only_for_web_watching() {
        let native = UserIdleShowState::default();
        assert_eq!(
            screen_lock_text(&native),
            format!("画面ロック: {NATIVE_LOCK_TEXT}")
        );
        let mut web = web_initial_state();
        assert_eq!(
            screen_lock_text(&web),
            format!("画面ロック: {UNKNOWN_TEXT}")
        );
        request_watch(&mut web);
        finish_request(
            &mut web,
            1,
            Ok(reading(UserPresenceView::Idle, ScreenLockView::Locked)),
        );
        assert_eq!(screen_lock_text(&web), "画面ロック: ロック中");
        assert!(presence_text(&web).contains("離席中"));
    }

    #[test]
    fn double_press_advances_generation_once() {
        let mut s = web_initial_state();
        request_watch(&mut s);
        request_watch(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(pending_request(&s), Some((1, WebRequestKind::Permission)));
    }

    #[test]
    fn stale_generation_is_ignored() {
        let mut s = web_initial_state();
        request_watch(&mut s);
        finish_request(&mut s, 0, Ok(active()));
        assert_eq!(s.view, IdleView::Requesting);
        apply_web_change(&mut s, 0, active());
        assert_eq!(s.view, IdleView::Requesting);
        finish_request(&mut s, 1, Ok(active()));
        assert!(matches!(s.view, IdleView::Watching(_)));
    }

    #[test]
    fn change_before_start_resolves_is_accepted() {
        let mut s = web_initial_state();
        request_watch(&mut s);
        apply_web_change(
            &mut s,
            1,
            reading(UserPresenceView::Idle, ScreenLockView::Unlocked),
        );
        assert!(matches!(s.view, IdleView::Watching(_)));
        finish_request(&mut s, 1, Ok(active()));
        assert!(matches!(s.view, IdleView::Watching(_)));
    }

    #[test]
    fn permission_denied_keeps_button_and_is_not_active() {
        let mut s = web_initial_state();
        request_watch(&mut s);
        finish_request(&mut s, 1, Err(ReadFailure::PermissionDenied));
        assert_eq!(s.view, IdleView::PermissionDenied);
        assert!(click_button(&current_screen(&s), REQUEST_LABEL, &mut s));
        assert_eq!(pending_request(&s), Some((2, WebRequestKind::Permission)));
    }

    #[test]
    fn pause_discards_late_completion_and_resume_only_from_paused() {
        let mut s = web_initial_state();
        request_watch(&mut s);
        finish_request(&mut s, 1, Ok(active()));
        pause_web(&mut s);
        assert_eq!(s.view, IdleView::Paused);
        assert_eq!(presence_text(&s), format!("在席: {UNDETERMINED_TEXT}"));
        // 古い世代の完了・変化は捨てる。
        finish_request(&mut s, 1, Ok(active()));
        apply_web_change(&mut s, 1, active());
        assert_eq!(s.view, IdleView::Paused);
        resume_web(&mut s);
        assert_eq!(pending_request(&s), Some((3, WebRequestKind::Resume)));
        // Paused 以外からは動かない。
        let before = s.clone();
        resume_web(&mut s);
        assert_eq!(s, before);
    }

    #[test]
    fn pause_while_requesting_returns_to_button() {
        let mut s = web_initial_state();
        request_watch(&mut s);
        pause_web(&mut s);
        assert_eq!(s.view, IdleView::AwaitingRequest);
        finish_request(&mut s, 1, Ok(active()));
        assert_eq!(s.view, IdleView::AwaitingRequest);
    }

    #[test]
    fn button_only_for_web_states() {
        let mut native = UserIdleShowState::default();
        assert!(!click_button(
            &current_screen(&native),
            REQUEST_LABEL,
            &mut native
        ));
        let mut web = web_initial_state();
        assert!(click_button(&current_screen(&web), REQUEST_LABEL, &mut web));
        assert_eq!(web.view, IdleView::Requesting);
    }

    #[test]
    fn render_headless_native_and_web() {
        let s = UserIdleShowState::default();
        let joined = render_headless(&current_screen(&s), &s).join("\n");
        assert!(joined.contains("読み取り中"));
        assert!(joined.contains("在席"));
        assert!(joined.contains("1 秒遅れ"));
        let w = web_initial_state();
        let joined = render_headless(&current_screen(&w), &w).join("\n");
        assert!(joined.contains(REQUEST_LABEL));
        assert!(joined.contains(WEB_ELAPSED_TEXT));
    }
}
