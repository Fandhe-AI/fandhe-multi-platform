//! Phase 11 の拡充ループ型サンプル「ワンタイムコードの入力」（親 #121、Issue #585、a）。
//! `FANDHE_PROXY_APP=otp_input` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod otp_input;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。ヒントの OS 呼び出しは effect ドライバ
//! （`device/autofill_otp.rs`）が [`take_hint_request`] で観測して行い、[`finish_hint`] で結果を注入する。
//! Web の WebOTP 待ち受け（「SMS を待つ」）は `fandhe-web-dom`（c、#587）が [`take_wait_request`] /
//! [`finish_wait`] で同じ形に乗せる。全 OS 分の状態・定数・文言は (a) で確定し、後続の OS 別 PR は
//! 本ファイルに触れない。
//!
//! # 結果を混同しない理由
//!
//! ヒントの成功は「OS に伝えた」までで、候補が出るかは分からない。成功文言に「有効」とは書かない。
//! 非対応・失敗は error 側に出す。キャンセルと期限切れは失敗ではないので status 側に中立文言で出す。
//! 世代番号で古い完了や待機外で届いた完了を無視する。コードの値は状態文・エラー文・`Debug` に出さない。

use crate::{Screen, WidgetId};

/// 「SMS を待つ」ボタンの `WidgetId`。
const WAIT_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「もう一度伝える」ボタンの `WidgetId`。
const RETRY_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 画面タイトル。
pub const TITLE: &str = "ワンタイムコードの入力";
/// 入力欄のラベル（Web ドライバが `<input>` を特定する手がかりにもなる）。
pub const CODE_FIELD_LABEL: &str = "ワンタイムコード";
/// 固定の注記。
pub const NOTE_TEXT: &str = "OS の自動入力にこの欄がワンタイムコードだと伝えます。候補が出るかは SMS の文面や OS の設定で決まり、\
アプリからは分かりません。iOS は数字キーボードになります。Android・macOS は保留中、Windows は非対応です。\
Web の Chromium 系は「SMS を待つ」で受け取れます（SMS の最終行が「@<ホスト名> #<コード>」の形のときだけ）";
/// ヒントを OS へ伝えている間の案内。
pub const HINT_APPLYING_STATUS: &str = "OS に伝えています";
/// ヒントを OS へ伝えた案内（候補が出るかは保証しない）。
pub const HINT_APPLIED_STATUS: &str =
    "OS に「ワンタイムコード」と伝えています（候補が出るかは OS が決めます）";
/// ヒントが非対応の案内。
pub const HINT_UNSUPPORTED_STATUS: &str =
    "この環境では自動入力のヒントを使えません（手入力でコードを入れられます）";
/// ヒント失敗の固定文言。
pub const HINT_FAILED_STATUS: &str = "OS に伝えられませんでした";
/// 「SMS を待つ」ボタンのラベル。
pub const WAIT_BUTTON_LABEL: &str = "SMS を待つ";
/// 「もう一度伝える」ボタンのラベル。
pub const RETRY_BUTTON_LABEL: &str = "もう一度伝える";
/// SMS 待ち受け中の案内。秒数は `fandhe_device::autofill_otp::WEBOTP_TIMEOUT_MS` と揃える（コアは device に依存できないため直書き）。
pub const WAITING_STATUS: &str = "SMS を待っています（最大 120 秒）";
/// コードを入れた案内（値は出さない）。
pub const RECEIVED_STATUS: &str = "SMS からコードを受け取り、入力欄に入れました";
/// コードを受け取ったが入力済みのため入れなかった案内。
pub const RECEIVED_NOT_FILLED_STATUS: &str = "入力済みのため上書きしませんでした";
/// 待ち受けが取り消された案内（中立）。
pub const WAIT_CANCELLED_STATUS: &str = "SMS の待ち受けを終了しました";
/// 待ち受けが期限切れになった案内（中立）。
pub const WAIT_TIMEOUT_STATUS: &str = "SMS が届きませんでした（手入力でコードを入れられます）";
/// 待ち受けが非対応の案内。
pub const WAIT_UNSUPPORTED_STATUS: &str = "この環境では SMS の待ち受けを使えません";
/// 待ち受け失敗の固定文言。
pub const WAIT_FAILED_STATUS: &str = "SMS を待てませんでした";

/// ヒントの進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HintPhase {
    /// 画面を開いた直後で、effect ドライバへまだ渡していない。
    #[default]
    Requested,
    /// effect ドライバが処理中（[`mark_hint_applying`] 済み）。
    Applying,
    /// OS へ伝えた。
    Applied,
    /// この環境では非対応。
    Unsupported,
    /// 伝えるのに失敗した。
    Failed,
}

/// ヒントの成功系の結果（写像は `fandhe-egui-proxy` 側の責務）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintOutcome {
    /// OS へ伝えた。
    Applied,
    /// この環境では非対応。
    Unsupported,
}

/// SMS 待ち受け（WebOTP）の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaitPhase {
    /// 未操作・前回の待ち受けが完了済み。
    #[default]
    Idle,
    /// ボタンが押され、ドライバへまだ渡していない。
    Requested,
    /// ドライバが待ち受け中（[`mark_waiting`] 済み）。
    Waiting,
}

/// 待ち受けの結果（写像は Web ドライバの責務）。
#[derive(Clone, PartialEq, Eq)]
pub enum CoreWaitOutcome {
    /// コードを受け取った。`fill` は適用時点の入力欄が空かどうか（`fandhe_device::autofill_otp::should_fill`）。
    Received {
        /// 受け取ったコード（加工しない）。
        code: String,
        /// 入力欄へ入れてよいか。false なら上書きしない。
        fill: bool,
    },
    /// 取り消された（中立）。
    Cancelled,
    /// 期限切れ（中立）。
    TimedOut,
}

impl std::fmt::Debug for CoreWaitOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Received { code, fill } => f
                .debug_struct("Received")
                .field("chars", &code.chars().count())
                .field("fill", fill)
                .finish(),
            Self::Cancelled => f.write_str("Cancelled"),
            Self::TimedOut => f.write_str("TimedOut"),
        }
    }
}

/// 待ち受けの失敗の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreWaitFailure {
    /// この環境では非対応。
    Unsupported,
    /// 失敗。
    Failed,
}

/// ヒントの要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HintRequest {
    /// [`finish_hint`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 待ち受けの要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitRequest {
    /// [`finish_wait`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 画面の状態。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct OtpInputState {
    /// 入力欄の値（加工せず保存する）。
    pub code: String,
    /// ヒントの進行状態。初期値 `Requested` で、画面を開いた最初の effect で 1 回だけ掛ける。
    pub hint: HintPhase,
    /// 直近のヒント要求の世代番号。
    pub hint_generation: u64,
    /// SMS 待ち受けを使えるか。Web ドライバが `OTPCredential` の有無を見て立てる。
    pub sms_wait_available: bool,
    /// 待ち受けの進行状態。
    pub wait: WaitPhase,
    /// 直近の待ち受け要求の世代番号。
    pub wait_generation: u64,
    /// 案内（通常色）。
    pub status: String,
    /// 非対応・失敗（`TextRole::Error`）。
    pub error: String,
}

impl std::fmt::Debug for OtpInputState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OtpInputState")
            .field("code_chars", &self.code.chars().count())
            .field("hint", &self.hint)
            .field("hint_generation", &self.hint_generation)
            .field("sms_wait_available", &self.sms_wait_available)
            .field("wait", &self.wait)
            .field("wait_generation", &self.wait_generation)
            .finish()
    }
}

/// effect ドライバが未発送のヒント要求を観測する（状態は変えない）。
pub fn take_hint_request(state: &OtpInputState) -> Option<HintRequest> {
    (state.hint == HintPhase::Requested).then_some(HintRequest {
        generation: state.hint_generation,
    })
}

/// ヒント要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_hint_applying(state: &mut OtpInputState) {
    if state.hint == HintPhase::Requested {
        state.hint = HintPhase::Applying;
        state.status = HINT_APPLYING_STATUS.to_string();
        state.error = String::new();
    }
}

/// ヒントの結果を反映する。`Applying` 以外・世代違いの完了は無視する。
pub fn finish_hint(
    state: &mut OtpInputState,
    generation: u64,
    result: Result<HintOutcome, String>,
) {
    if state.hint != HintPhase::Applying || state.hint_generation != generation {
        return;
    }
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(HintOutcome::Applied) => {
            state.hint = HintPhase::Applied;
            state.status = HINT_APPLIED_STATUS.to_string();
        }
        Ok(HintOutcome::Unsupported) => {
            state.hint = HintPhase::Unsupported;
            state.error = HINT_UNSUPPORTED_STATUS.to_string();
        }
        Err(m) => {
            state.hint = HintPhase::Failed;
            state.error = m;
        }
    }
}

/// 「もう一度伝える」ボタンのハンドラ。`Failed` のときだけ受け付け、世代を進めて再要求する。
pub fn retry_hint(state: &mut OtpInputState) {
    if state.hint != HintPhase::Failed {
        return;
    }
    state.hint = HintPhase::Requested;
    state.hint_generation = state.hint_generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// SMS 待ち受けを使えるかを設定する（Web ドライバが起動時に呼ぶ）。
pub fn set_sms_wait_available(state: &mut OtpInputState, available: bool) {
    state.sms_wait_available = available;
}

/// 「SMS を待つ」ボタンのハンドラ。使える環境で `Idle` のときだけ受け付ける（二重押し防止）。
pub fn start_wait(state: &mut OtpInputState) {
    if !state.sms_wait_available || state.wait != WaitPhase::Idle {
        return;
    }
    state.wait = WaitPhase::Requested;
    state.wait_generation = state.wait_generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// ドライバが未発送の待ち受け要求を観測する（状態は変えない）。
pub fn take_wait_request(state: &OtpInputState) -> Option<WaitRequest> {
    (state.wait == WaitPhase::Requested).then_some(WaitRequest {
        generation: state.wait_generation,
    })
}

/// 待ち受け要求を待機中へ進める。`Requested` 以外では何もしない。
pub fn mark_waiting(state: &mut OtpInputState) {
    if state.wait == WaitPhase::Requested {
        state.wait = WaitPhase::Waiting;
        state.status = WAITING_STATUS.to_string();
    }
}

/// 待ち受けの結果を反映する。`Waiting` 以外・世代違いの完了は無視する。
pub fn finish_wait(
    state: &mut OtpInputState,
    generation: u64,
    result: Result<CoreWaitOutcome, CoreWaitFailure>,
) {
    if state.wait != WaitPhase::Waiting || state.wait_generation != generation {
        return;
    }
    state.wait = WaitPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreWaitOutcome::Received { code, fill }) => {
            if fill {
                state.code = code;
                state.status = RECEIVED_STATUS.to_string();
            } else {
                state.status = RECEIVED_NOT_FILLED_STATUS.to_string();
            }
        }
        Ok(CoreWaitOutcome::Cancelled) => state.status = WAIT_CANCELLED_STATUS.to_string(),
        Ok(CoreWaitOutcome::TimedOut) => state.status = WAIT_TIMEOUT_STATUS.to_string(),
        Err(CoreWaitFailure::Unsupported) => state.error = WAIT_UNSUPPORTED_STATUS.to_string(),
        Err(CoreWaitFailure::Failed) => state.error = WAIT_FAILED_STATUS.to_string(),
    }
}

/// 入力欄の表示値（`text_field` の `get`）。
pub fn code_text(state: &OtpInputState) -> String {
    state.code.clone()
}

/// 入力欄の入力を保存する（`text_field` の `set`）。trim などの加工をしない。
pub fn set_code(state: &mut OtpInputState, value: String) {
    state.code = value;
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &OtpInputState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &OtpInputState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。待ち受けボタンは使える環境の `Idle` のときだけ、再試行ボタンは `Failed` のときだけ出す。
pub fn current_screen(state: &OtpInputState) -> Screen<OtpInputState> {
    let mut screen = Screen::new(TITLE)
        .text(NOTE_TEXT)
        .text_field(CODE_FIELD_LABEL, code_text, set_code)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.sms_wait_available && state.wait == WaitPhase::Idle {
        screen = screen.button(WAIT_BUTTON_ID, WAIT_BUTTON_LABEL, start_wait);
    }
    if state.hint == HintPhase::Failed {
        screen = screen.button(RETRY_BUTTON_ID, RETRY_BUTTON_LABEL, retry_hint);
    }
    screen
}

/// 操作ボタンを常に含む固定の画面（Web 向け）。
///
/// Web の描画は Screen を一度だけ組んで固定し、状態だけを差し替えて再描画する。
/// [`current_screen`] は `sms_wait_available` が立ってからでないと「SMS を待つ」を含めないため、
/// 起動後に対応可否が確定する Web では到達できない。ここではボタンを最初から含め、
/// 使えるかどうかは各ハンドラ（[`start_wait`] / [`retry_hint`]）が状態を見て判断する。
pub fn fixed_screen() -> Screen<OtpInputState> {
    Screen::new(TITLE)
        .text(NOTE_TEXT)
        .text_field(CODE_FIELD_LABEL, code_text, set_code)
        .dynamic_text(status_text)
        .error_text(error_text)
        .button(WAIT_BUTTON_ID, WAIT_BUTTON_LABEL, start_wait)
        .button(RETRY_BUTTON_ID, RETRY_BUTTON_LABEL, retry_hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_screen_has_both_buttons_and_handlers_gate() {
        let screen = fixed_screen();
        assert_eq!(screen.title(), TITLE);
        let mut s = OtpInputState::default();
        start_wait(&mut s);
        retry_hint(&mut s);
        assert_eq!(s.wait, WaitPhase::Idle);
        assert_eq!(s.hint, HintPhase::Requested);
        assert_eq!(s.hint_generation, 0);
    }

    fn applying() -> OtpInputState {
        let mut s = OtpInputState::default();
        mark_hint_applying(&mut s);
        s
    }

    fn waiting() -> OtpInputState {
        let mut s = OtpInputState::default();
        set_sms_wait_available(&mut s, true);
        start_wait(&mut s);
        mark_waiting(&mut s);
        s
    }

    #[test]
    fn hint_is_requested_once_on_open() {
        let mut s = OtpInputState::default();
        assert_eq!(take_hint_request(&s).unwrap().generation, 0);
        mark_hint_applying(&mut s);
        assert!(take_hint_request(&s).is_none());
    }

    #[test]
    fn hint_outcomes_are_distinct() {
        let mut ok = applying();
        finish_hint(&mut ok, 0, Ok(HintOutcome::Applied));
        assert_eq!(ok.hint, HintPhase::Applied);
        assert!(!ok.status.is_empty() && ok.error.is_empty());
        assert!(!ok.status.contains("有効"));

        let mut un = applying();
        finish_hint(&mut un, 0, Ok(HintOutcome::Unsupported));
        assert!(un.status.is_empty() && !un.error.is_empty());

        let mut ng = applying();
        finish_hint(&mut ng, 0, Err(HINT_FAILED_STATUS.into()));
        assert_eq!(ng.hint, HintPhase::Failed);
        assert!(ng.status.is_empty() && !ng.error.is_empty());
        assert_ne!(un.error, ng.error);
    }

    #[test]
    fn stale_or_misphased_hint_completion_is_ignored() {
        let mut s = applying();
        finish_hint(&mut s, 1, Ok(HintOutcome::Applied));
        assert_eq!(s.hint, HintPhase::Applying);

        let mut fresh = OtpInputState::default();
        finish_hint(&mut fresh, 0, Ok(HintOutcome::Applied));
        assert_eq!(fresh.hint, HintPhase::Requested);
    }

    #[test]
    fn retry_only_from_failed() {
        let mut s = applying();
        retry_hint(&mut s);
        assert_eq!(s.hint, HintPhase::Applying);
        finish_hint(&mut s, 0, Err(HINT_FAILED_STATUS.into()));
        retry_hint(&mut s);
        assert_eq!((s.hint, s.hint_generation), (HintPhase::Requested, 1));
        assert!(s.error.is_empty());
    }

    #[test]
    fn start_wait_requires_availability_and_idle() {
        let mut s = OtpInputState::default();
        start_wait(&mut s);
        assert_eq!(s.wait, WaitPhase::Idle);
        set_sms_wait_available(&mut s, true);
        start_wait(&mut s);
        assert_eq!((s.wait, s.wait_generation), (WaitPhase::Requested, 1));
        start_wait(&mut s);
        assert_eq!(s.wait_generation, 1);
        assert_eq!(take_wait_request(&s).unwrap().generation, 1);
        mark_waiting(&mut s);
        assert!(take_wait_request(&s).is_none());
        start_wait(&mut s);
        assert_eq!(s.wait_generation, 1);
    }

    #[test]
    fn received_fills_only_when_allowed() {
        let mut s = waiting();
        finish_wait(
            &mut s,
            1,
            Ok(CoreWaitOutcome::Received {
                code: " 123456 ".into(),
                fill: true,
            }),
        );
        assert_eq!(s.code, " 123456 ");
        assert_eq!(s.wait, WaitPhase::Idle);

        let mut kept = waiting();
        set_code(&mut kept, "99".into());
        finish_wait(
            &mut kept,
            1,
            Ok(CoreWaitOutcome::Received {
                code: "123456".into(),
                fill: false,
            }),
        );
        assert_eq!(kept.code, "99");
        assert_eq!(kept.status, RECEIVED_NOT_FILLED_STATUS);
    }

    #[test]
    fn cancel_and_timeout_are_neutral() {
        for r in [CoreWaitOutcome::Cancelled, CoreWaitOutcome::TimedOut] {
            let mut s = waiting();
            finish_wait(&mut s, 1, Ok(r));
            assert!(s.error.is_empty() && !s.status.is_empty());
            assert!(s.code.is_empty());
        }
    }

    #[test]
    fn wait_failures_go_to_error_side() {
        for f in [CoreWaitFailure::Unsupported, CoreWaitFailure::Failed] {
            let mut s = waiting();
            finish_wait(&mut s, 1, Err(f));
            assert!(s.status.is_empty() && !s.error.is_empty());
            assert!(s.code.is_empty());
        }
    }

    #[test]
    fn stale_or_idle_wait_completion_is_ignored() {
        let mut s = waiting();
        finish_wait(&mut s, 2, Err(CoreWaitFailure::Failed));
        assert_eq!(s.wait, WaitPhase::Waiting);

        let mut idle = OtpInputState::default();
        finish_wait(&mut idle, 0, Ok(CoreWaitOutcome::Cancelled));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn code_is_stored_raw_and_never_shown() {
        let mut s = OtpInputState::default();
        set_code(&mut s, " １２３ 456 ".into());
        assert_eq!(code_text(&s), " １２３ 456 ");
        assert!(!format!("{s:?}").contains("456"));
        let o = CoreWaitOutcome::Received {
            code: "654321".into(),
            fill: true,
        };
        assert!(!format!("{o:?}").contains("654321"));
        let mut w = waiting();
        finish_wait(&mut w, 1, Ok(o));
        assert!(!w.status.contains("654321") && !w.error.contains("654321"));
    }
}
