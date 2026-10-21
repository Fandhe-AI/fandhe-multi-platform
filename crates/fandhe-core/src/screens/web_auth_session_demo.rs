//! Phase 11（端末機能の拡充ループ、親 #113）型サンプル「OAuth 用の Web 認証セッション」（Issue #530）。
//! 「認可画面を開く」「取り消す」の 2 ボタンだけの 1 画面。`FANDHE_PROXY_APP=web_auth_session_demo` で
//! 選ばれ、`fandhe-egui-proxy` の `screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod web_auth_session_demo;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS の認証 UI・デモ認可サーバー・
//! コールバック URL の検査は `fandhe-egui-proxy` の effect ドライバ（`device/web_auth_session.rs`）が
//! [`take_request`] で要求を観測して行い、[`finish`] で結果を注入する。全 OS 分の状態・文言は
//! ここで定義し、後続の OS 別 issue は画面に触らない。
//!
//! # 認可の成立と言わない理由
//!
//! デモ認可ページに実在のサービスはなく、トークン交換もしない。「URL が返った」を「認可された」と
//! 表示すると誤解させるため、固定の注意書きを常に出す。キャンセル・期限切れは失敗に見せない
//! （通常色）。世代番号（`generation`）で古い完了や `Idle` で届いた完了を無視する
//! （`passkey_signin` と同じ判断）。URL は伏せ字（state・code の値なし）だけを持つ。

use crate::{Screen, WidgetId};

/// 常に表示する注意書き。
pub const DEMO_NOTICE: &str = "アプリ内のデモ認可ページを開きます。トークン交換はしません。この結果は認可の成立を意味しません";
/// 認証 UI を開いている間の案内（ブラウザで開く OS 向けの戻り方を含む）。
pub const RUNNING_STATUS: &str = "認可画面で「許可する」か「拒否する」を選んでください（ブラウザで開いた場合は、選んだあとにこの画面へ戻ってください）";
/// 認可コード付きの URL が返ったときの案内。
pub const RETURNED_CODE_STATUS: &str =
    "認可コード付きのコールバック URL が返りました（認可の成立ではありません）";
/// 拒否の URL が返ったときの案内（失敗ではない）。
pub const RETURNED_DENIED_STATUS: &str = "拒否を示すコールバック URL が返りました";
/// 取り消したときの案内（通常色）。
pub const CANCELLED_STATUS: &str = "認可画面を閉じました";
/// 期限切れのときの案内（通常色）。
pub const TIMED_OUT_STATUS: &str = "時間内に完了しなかったため終了しました";
/// 非対応環境の案内（error 欄）。
pub const UNSUPPORTED_TEXT: &str = "この環境では Web 認証セッションを使えません";
/// ブラウザなしの案内（error 欄）。
pub const NO_BROWSER_TEXT: &str = "認可画面を開けるブラウザがありません";
/// 不正なコールバックの案内（error 欄）。
pub const INVALID_CALLBACK_TEXT: &str =
    "返ってきた URL が今回の要求と一致しないため、成功として扱いません";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 内部エラーの固定文言（OS の値は出さない）。
pub const INTERNAL_TEXT: &str = "認可画面を完了できませんでした";

/// 「認可画面を開く」ボタンの `WidgetId`。
pub const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「取り消す」ボタンの `WidgetId`。
pub const CANCEL_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_running`] 済み）。
    Running,
}

/// 返ってきた URL の中身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnedKind {
    /// 認可コードが付いていた（値は持たず長さだけ）。
    Code {
        /// 文字数。
        len: usize,
    },
    /// 認可サーバーがエラー（拒否など）を返した。
    Denied {
        /// エラーコード。
        error: String,
    },
}

/// 認証セッションの結果（`fandhe_device::web_auth_session::WebAuthOutcome` の写し）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreOutcome {
    /// state 一致の URL が返った。
    Returned {
        /// 伏せ字の URL。
        masked_url: String,
        /// 中身。
        kind: ReturnedKind,
    },
    /// 利用者の操作で閉じた。
    Cancelled,
    /// 期限切れ。
    TimedOut,
}

/// 失敗の種別（`WebAuthFailure` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFailure {
    /// この環境では使えない。
    Unsupported,
    /// ブラウザなし。
    NoBrowser,
    /// 返ってきた URL が検査に通らない。
    InvalidCallback,
    /// その他。
    Internal,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebAuthSessionDemoState {
    /// 進行状態。
    pub phase: Phase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 処理中に取り消しが押された。
    pub cancel_requested: bool,
    /// 結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
    /// 詳細行（伏せ字の URL と PKCE）。
    pub detail: String,
}

/// 「認可画面を開く」のハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn start(state: &mut WebAuthSessionDemoState) {
    if state.phase != Phase::Idle {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.cancel_requested = false;
    state.status = String::new();
    state.error = String::new();
    state.detail = String::new();
}

/// 「取り消す」のハンドラ。`Running` 以外では no-op。
pub fn cancel(state: &mut WebAuthSessionDemoState) {
    if state.phase == Phase::Running {
        state.cancel_requested = true;
    }
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。戻り値は世代番号。
pub fn take_request(state: &WebAuthSessionDemoState) -> Option<u64> {
    (state.phase == Phase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut WebAuthSessionDemoState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Running;
        state.status = RUNNING_STATUS.to_string();
    }
}

/// effect ドライバが取り消しの押下を観測する（状態は変えない）。戻り値は世代番号。
pub fn take_cancel(state: &WebAuthSessionDemoState) -> Option<u64> {
    (state.phase == Phase::Running && state.cancel_requested).then_some(state.generation)
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish(
    state: &mut WebAuthSessionDemoState,
    generation: u64,
    result: Result<CoreOutcome, CoreFailure>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.cancel_requested = false;
    state.status = String::new();
    state.error = String::new();
    state.detail = String::new();
    match result {
        Ok(CoreOutcome::Returned { masked_url, kind }) => {
            state.status = match kind {
                ReturnedKind::Code { .. } => RETURNED_CODE_STATUS,
                ReturnedKind::Denied { .. } => RETURNED_DENIED_STATUS,
            }
            .to_string();
            state.detail = format!("{masked_url}\nPKCE: S256");
        }
        Ok(CoreOutcome::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Ok(CoreOutcome::TimedOut) => state.status = TIMED_OUT_STATUS.to_string(),
        Err(CoreFailure::Unsupported) => state.error = UNSUPPORTED_TEXT.to_string(),
        Err(CoreFailure::NoBrowser) => state.error = NO_BROWSER_TEXT.to_string(),
        Err(CoreFailure::InvalidCallback) => state.error = INVALID_CALLBACK_TEXT.to_string(),
        Err(CoreFailure::Internal) => state.error = format!("{FAILED_PREFIX}{INTERNAL_TEXT}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &WebAuthSessionDemoState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &WebAuthSessionDemoState) -> String {
    state.error.clone()
}

/// 詳細文言（`DynamicText` の `get`）。
pub fn detail_text(state: &WebAuthSessionDemoState) -> String {
    state.detail.clone()
}

/// 画面を組み立てる。注意書きは常に出し、処理中は「取り消す」だけを出して同時に 2 件開かない。
pub fn current_screen(state: &WebAuthSessionDemoState) -> Screen<WebAuthSessionDemoState> {
    let screen = Screen::new("Web 認証セッション")
        .text(DEMO_NOTICE)
        .dynamic_text(status_text)
        .dynamic_text(detail_text)
        .error_text(error_text);
    match state.phase {
        Phase::Idle => screen.button(START_BUTTON_ID, "認可画面を開く", start),
        Phase::Requested => screen,
        Phase::Running => screen.button(CANCEL_BUTTON_ID, "取り消す", cancel),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> WebAuthSessionDemoState {
        let mut s = WebAuthSessionDemoState::default();
        start(&mut s);
        mark_running(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, CoreFailure>) -> WebAuthSessionDemoState {
        let mut s = running();
        let g = s.generation;
        finish(&mut s, g, r);
        assert_eq!(s.phase, Phase::Idle);
        s
    }

    #[test]
    fn double_start_is_ignored_and_generation_advances_once() {
        let mut s = WebAuthSessionDemoState::default();
        start(&mut s);
        start(&mut s);
        assert_eq!((s.generation, take_request(&s)), (1, Some(1)));
        mark_running(&mut s);
        assert_eq!(take_request(&s), None);
        start(&mut s);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn cancel_only_while_running() {
        let mut s = WebAuthSessionDemoState::default();
        cancel(&mut s);
        assert!(!s.cancel_requested);
        start(&mut s);
        cancel(&mut s);
        assert_eq!(take_cancel(&s), None);
        mark_running(&mut s);
        cancel(&mut s);
        assert_eq!(take_cancel(&s), Some(1));
        let g = s.generation;
        finish(&mut s, g, Ok(CoreOutcome::Cancelled));
        assert!(!s.cancel_requested);
    }

    #[test]
    fn stale_or_idle_completion_is_ignored() {
        let mut s = running();
        let g = s.generation;
        finish(&mut s, g + 1, Ok(CoreOutcome::TimedOut));
        assert_eq!(s.phase, Phase::Running);
        finish(&mut s, g, Ok(CoreOutcome::TimedOut));
        let after = s.clone();
        finish(&mut s, g, Err(CoreFailure::Internal));
        assert_eq!(s, after);
    }

    #[test]
    fn cancelled_and_timed_out_are_not_errors() {
        for (r, text) in [
            (CoreOutcome::Cancelled, CANCELLED_STATUS),
            (CoreOutcome::TimedOut, TIMED_OUT_STATUS),
        ] {
            let s = shown(Ok(r));
            assert_eq!(s.status, text);
            assert!(s.error.is_empty() && s.detail.is_empty());
        }
    }

    #[test]
    fn failures_go_to_error_only() {
        for (f, text) in [
            (CoreFailure::Unsupported, UNSUPPORTED_TEXT),
            (CoreFailure::NoBrowser, NO_BROWSER_TEXT),
            (CoreFailure::InvalidCallback, INVALID_CALLBACK_TEXT),
        ] {
            let s = shown(Err(f));
            assert_eq!(s.error, text);
            assert!(s.status.is_empty() && s.detail.is_empty());
        }
        let s = shown(Err(CoreFailure::Internal));
        assert!(s.error.starts_with(FAILED_PREFIX) && s.status.is_empty());
    }

    #[test]
    fn returned_shows_masked_url_and_pkce() {
        let s = shown(Ok(CoreOutcome::Returned {
            masked_url: "x?state=（一致）&code=（43 文字）".into(),
            kind: ReturnedKind::Code { len: 43 },
        }));
        assert_eq!(s.status, RETURNED_CODE_STATUS);
        assert!(s.detail.contains("PKCE: S256") && s.error.is_empty());
        let d = shown(Ok(CoreOutcome::Returned {
            masked_url: "x".into(),
            kind: ReturnedKind::Denied {
                error: "access_denied".into(),
            },
        }));
        assert_eq!(d.status, RETURNED_DENIED_STATUS);
        assert!(d.error.is_empty());
    }

    #[test]
    fn start_clears_previous_display() {
        let mut s = shown(Err(CoreFailure::Unsupported));
        start(&mut s);
        assert!(s.error.is_empty());
    }
}
