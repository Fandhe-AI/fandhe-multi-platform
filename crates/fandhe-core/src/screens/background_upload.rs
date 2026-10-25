//! Phase 11 の拡充ループ（親 #170、Issue #835）型サンプル「バックグラウンド送信」。
//! 「送信を始める」「取り消し」の 2 ボタンと状態 1 行を持つ 1 画面。`FANDHE_PROXY_APP=background_upload`
//! で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod background_upload;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは `fandhe-egui-proxy` の
//! effect ドライバ（`device/background_upload.rs`）が [`take_request`] で要求を観測し
//! [`finish_action`] で結果を注入する（`background_download.rs` と同型の状態機械）。全 OS 分の文言と
//! 状態の型はここに置き、OS ごとの子 issue はこの画面を触らない。
//!
//! 送信は OS が利用者の操作なしに進めるため、状態の SSOT は OS の一覧だけとし、ドライバが画面を開いたとき
//! （[`needs_read`]）と OS の合図のたびに読み直して [`apply_snapshot`] を呼ぶ。周期のポーリングはしない。
//! [`finish_action`] は `phase` を戻して操作の失敗を出すだけで [`UploadView`] は変えない
//! （完了通知と観測の食い違いを構造的に作らない）。
//!
//! ```text
//! Idle --「開始」/「取り消し」--> Requested(Action), generation += 1
//!   --take_request → mark_in_flight--> InFlight
//!   --finish_action（世代が一致するときだけ）--> Idle（Err なら error を表示）
//! ```

use crate::{Screen, WidgetId};

const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const CANCEL_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 画面の見出し。
pub const TITLE: &str = "バックグラウンド送信";
/// 説明文。
pub const DESCRIPTION: &str =
    "アプリが作る 1 MiB のファイルを OS に送ってもらいます。アプリを閉じても続くかは OS によります";
/// 送信先の案内（URL・パスは出さない）。
pub const DESTINATION: &str = "送信先: 確認用の受け口（serve-upload-sink.mjs）";
/// 未開始。
pub const IDLE: &str = "まだ送っていません";
/// 待機中。
pub const WAITING: &str = "送信を待っています";
/// 取り消し済み（失敗ではない）。
pub const CANCELLED: &str = "取り消しました";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗: ";
/// 状態の読み出し失敗（未開始と区別する）。
pub const READ_FAILED: &str = "状態を読み出せません";
/// 操作の失敗の接頭辞。
pub const ACTION_FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応環境の固定文言。
pub const UNSUPPORTED: &str = "この環境ではバックグラウンド送信を使えません";

/// ボタン操作の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 「送信を始める」。
    Start,
    /// 「取り消し」。
    Cancel,
}

/// ボタン操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、ドライバへまだ渡していない。
    Requested(Action),
    /// ドライバが OS 呼び出しを処理中。
    InFlight,
}

/// 失敗理由の表示用写像（`fandhe_device::background_upload::UploadFailure` と対応。
/// `fandhe-core` は `fandhe-device` に依存しないので写像は proxy 側の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureView {
    /// HTTP の状態が 2xx でない。
    Http(u16),
    /// 送信バイト数がファイルの大きさと違う。
    SizeMismatch {
        /// 送信できたバイト数。
        sent: u64,
        /// 期待したバイト数。
        expected: u64,
    },
    /// 接続の失敗。
    Network,
    /// ファイルを用意できない。
    FileUnavailable,
    /// 許可外のリダイレクト先。
    RedirectNotAllowed,
    /// その他。
    Other,
}

/// OS 側の送信状態の表示用写像。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UploadView {
    /// まだ観測していない。
    #[default]
    Unknown,
    /// この環境は非対応（`Unknown` と分ける。分けないと開いたときの読み直しが止まらない）。
    Unsupported,
    /// 未開始。
    Idle,
    /// 待機中。
    Waiting,
    /// 送信中。
    Sending {
        /// 送信済みバイト数。
        sent: u64,
        /// 総バイト数（不明なら `None`）。
        total: Option<u64>,
    },
    /// 完了。
    Completed {
        /// 送信したバイト数。
        bytes: u64,
        /// HTTP の状態。
        http_status: u16,
    },
    /// 取り消し。
    Cancelled,
    /// 失敗。
    Failed(FailureView),
    /// 状態の読み出しに失敗した。
    ReadFailed,
}

/// 操作（開始・取り消し）の失敗理由。文言は固定（A09）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionFailure {
    /// この環境は非対応。
    Unsupported,
    /// 送信先が許可されていない。
    InvalidDestination,
    /// 送るファイルを用意できない。
    FileUnavailable,
    /// 開始できなかった。
    StartFailed,
    /// 取り消しを OS に伝えられなかった。
    CancelFailed,
}

impl ActionFailure {
    fn message(self) -> &'static str {
        match self {
            Self::Unsupported => UNSUPPORTED,
            Self::InvalidDestination => "送信先が許可されていません",
            Self::FileUnavailable => "送るファイルを用意できませんでした",
            Self::StartFailed => "開始できませんでした",
            Self::CancelFailed => "取り消しを OS に伝えられませんでした",
        }
    }
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackgroundUploadState {
    /// ボタン操作の進行状態。
    pub phase: Phase,
    /// 直近の要求の世代。完了通知が「今の要求」のものかの照合に使う。
    pub generation: u64,
    /// OS 側の送信状態（OS の一覧を読み直して更新）。
    pub view: UploadView,
    /// 操作の失敗理由（成功時は `None`）。
    pub error: Option<ActionFailure>,
}

/// ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// [`finish_action`] へそのまま渡す世代。
    pub generation: u64,
    /// 実行する操作。
    pub action: Action,
}

fn request(state: &mut BackgroundUploadState, action: Action) {
    state.phase = Phase::Requested(action);
    state.generation = state.generation.wrapping_add(1);
    state.error = None;
}

fn is_active(view: &UploadView) -> bool {
    matches!(view, UploadView::Waiting | UploadView::Sending { .. })
}

/// 「送信を始める」。処理中、または待機中・送信中は二重要求を防ぐため no-op。
pub fn start(state: &mut BackgroundUploadState) {
    if state.phase != Phase::Idle || is_active(&state.view) {
        return;
    }
    request(state, Action::Start);
}

/// 「取り消し」。処理中は no-op。何も無いときも出す（契約上 `Ok`）。
pub fn cancel(state: &mut BackgroundUploadState) {
    if state.phase != Phase::Idle {
        return;
    }
    request(state, Action::Cancel);
}

/// ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &BackgroundUploadState) -> Option<Request> {
    match state.phase {
        Phase::Requested(action) => Some(Request {
            generation: state.generation,
            action,
        }),
        _ => None,
    }
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut BackgroundUploadState) {
    if matches!(state.phase, Phase::Requested(_)) {
        state.phase = Phase::InFlight;
    }
}

/// 操作の結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は捨てる。
/// `Err` は失敗を出すだけで、送信状態（[`UploadView`]）は変えない。
pub fn finish_action(
    state: &mut BackgroundUploadState,
    generation: u64,
    result: Result<(), ActionFailure>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.error = result.err();
}

/// OS 側の送信状態を反映する（`phase` には触れない）。冪等。
pub fn apply_snapshot(state: &mut BackgroundUploadState, view: UploadView) {
    state.view = view;
}

/// 画面を開いた直後など、まだ OS の一覧を読んでいないか。ドライバが最初の読み出しを積む判断に使う。
pub fn needs_read(state: &BackgroundUploadState) -> bool {
    state.view == UploadView::Unknown
}

fn failure_text(failure: &FailureView) -> String {
    match failure {
        FailureView::Http(code) => format!("HTTP {code}"),
        FailureView::SizeMismatch { sent, expected } => {
            format!("送信できたのは {sent} / {expected} バイトです")
        }
        FailureView::Network => "接続できませんでした".to_string(),
        FailureView::FileUnavailable => "送るファイルを用意できませんでした".to_string(),
        FailureView::RedirectNotAllowed => "リダイレクト先が許可されていません".to_string(),
        FailureView::Other => "送信に失敗しました".to_string(),
    }
}

/// 状態 1 行の文言。`total == 0` は割り算せずバイト数だけ、送信が総数を超えても 100% で止める。
/// 失敗・読み出しの失敗・非対応は空（エラー行 [`error_text`] に出す）。
pub fn format_status(view: &UploadView) -> String {
    match view {
        UploadView::Unknown | UploadView::Idle => IDLE.to_string(),
        UploadView::Waiting => WAITING.to_string(),
        UploadView::Sending {
            sent,
            total: Some(total),
        } if *total > 0 => {
            let percent = (u128::from(*sent.min(total)) * 100 / u128::from(*total)).min(100);
            format!("送信中 {sent} / {total} バイト（{percent}%）")
        }
        UploadView::Sending { sent, .. } => format!("送信中 {sent} バイト"),
        UploadView::Completed { bytes, http_status } => {
            format!("送りました: {bytes} バイト（HTTP {http_status}）")
        }
        UploadView::Cancelled => CANCELLED.to_string(),
        UploadView::Unsupported | UploadView::Failed(_) | UploadView::ReadFailed => String::new(),
    }
}

fn status_text(state: &BackgroundUploadState) -> String {
    format_status(&state.view)
}

/// エラー行。操作の失敗を優先し、無ければ送信の失敗・読み出しの失敗・非対応を出す。
/// 取り消しは失敗の色で出さない。
pub fn error_line(state: &BackgroundUploadState) -> String {
    if let Some(failure) = state.error {
        return format!("{ACTION_FAILED_PREFIX}{}", failure.message());
    }
    match &state.view {
        UploadView::Failed(f) => format!("{FAILED_PREFIX}{}", failure_text(f)),
        UploadView::ReadFailed => READ_FAILED.to_string(),
        UploadView::Unsupported => UNSUPPORTED.to_string(),
        _ => String::new(),
    }
}

fn error_text(state: &BackgroundUploadState) -> String {
    error_line(state)
}

/// 画面を組み立てる。処理中は二重押しを防ぐためボタンを外し、待機中・送信中は開始ボタンも外す。
pub fn current_screen(state: &BackgroundUploadState) -> Screen<BackgroundUploadState> {
    let screen = Screen::new(TITLE)
        .text(DESCRIPTION)
        .text(DESTINATION)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != Phase::Idle {
        return screen;
    }
    let screen = if is_active(&state.view) {
        screen
    } else {
        screen.button(START_BUTTON_ID, "送信を始める", start)
    };
    screen.button(CANCEL_BUTTON_ID, "取り消し", cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn in_flight(state: &mut BackgroundUploadState, f: fn(&mut BackgroundUploadState)) {
        f(state);
        mark_in_flight(state);
    }

    #[test]
    fn start_and_cancel_advance_generation() {
        let mut s = BackgroundUploadState::default();
        start(&mut s);
        assert_eq!(s.phase, Phase::Requested(Action::Start));
        assert_eq!(s.generation, 1);
        assert_eq!(
            take_request(&s),
            Some(Request {
                generation: 1,
                action: Action::Start
            })
        );
        mark_in_flight(&mut s);
        assert_eq!(take_request(&s), None);
        finish_action(&mut s, 1, Ok(()));
        cancel(&mut s);
        assert_eq!(s.generation, 2);
    }

    #[test]
    fn start_is_noop_while_active_or_in_flight() {
        for view in [
            UploadView::Waiting,
            UploadView::Sending {
                sent: 1,
                total: None,
            },
        ] {
            let mut s = BackgroundUploadState {
                view,
                ..Default::default()
            };
            start(&mut s);
            assert_eq!(s.phase, Phase::Idle);
        }
        let mut s = BackgroundUploadState::default();
        in_flight(&mut s, start);
        start(&mut s);
        cancel(&mut s);
        assert_eq!((s.phase, s.generation), (Phase::InFlight, 1));
    }

    #[test]
    fn stale_generation_and_idle_completions_are_dropped() {
        let mut s = BackgroundUploadState::default();
        in_flight(&mut s, start);
        finish_action(&mut s, 7, Err(ActionFailure::StartFailed));
        assert_eq!(s.phase, Phase::InFlight);
        assert_eq!(s.error, None);
        finish_action(&mut s, 1, Ok(()));
        finish_action(&mut s, 1, Err(ActionFailure::StartFailed));
        assert_eq!(s.error, None);
    }

    #[test]
    fn err_completion_shows_error_and_keeps_view() {
        let mut s = BackgroundUploadState::default();
        in_flight(&mut s, start);
        finish_action(&mut s, 1, Err(ActionFailure::Unsupported));
        assert!(error_line(&s).starts_with(ACTION_FAILED_PREFIX));
        assert_eq!(s.view, UploadView::Unknown);
        assert_eq!(format_status(&s.view), IDLE);
    }

    #[test]
    fn successful_cancel_leaves_no_error() {
        let mut s = BackgroundUploadState::default();
        in_flight(&mut s, cancel);
        finish_action(&mut s, 1, Ok(()));
        assert_eq!(error_line(&s), "");
    }

    #[test]
    fn cancelled_is_not_an_error_but_failed_is() {
        let mut s = BackgroundUploadState {
            view: UploadView::Cancelled,
            ..Default::default()
        };
        assert_eq!(error_line(&s), "");
        assert_eq!(format_status(&s.view), CANCELLED);
        s.view = UploadView::Failed(FailureView::Http(404));
        assert_eq!(error_line(&s), "失敗: HTTP 404");
        s.view = UploadView::ReadFailed;
        assert_eq!(error_line(&s), READ_FAILED);
        s.view = UploadView::Unsupported;
        assert_eq!(error_line(&s), UNSUPPORTED);
    }

    #[test]
    fn needs_read_only_before_first_observation() {
        let mut s = BackgroundUploadState::default();
        assert!(needs_read(&s));
        apply_snapshot(&mut s, UploadView::Unsupported);
        assert!(!needs_read(&s));
        apply_snapshot(&mut s, UploadView::Idle);
        assert!(!needs_read(&s));
    }

    #[test]
    fn format_status_arms() {
        let r = |sent, total| format_status(&UploadView::Sending { sent, total });
        assert_eq!(r(20, Some(40)), "送信中 20 / 40 バイト（50%）");
        assert_eq!(r(20, None), "送信中 20 バイト");
        assert_eq!(r(20, Some(0)), "送信中 20 バイト");
        assert_eq!(r(50, Some(40)), "送信中 50 / 40 バイト（100%）");
        assert!(r(u64::MAX, Some(u64::MAX)).ends_with("（100%）"));
        assert_eq!(
            format_status(&UploadView::Completed {
                bytes: 1_048_576,
                http_status: 200
            }),
            "送りました: 1048576 バイト（HTTP 200）"
        );
        assert_eq!(format_status(&UploadView::Waiting), WAITING);
    }

    #[test]
    fn buttons_follow_state() {
        let mut s = BackgroundUploadState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, "送信を始める", &mut s));
        assert_eq!(s.phase, Phase::Requested(Action::Start));

        let mut s = BackgroundUploadState {
            phase: Phase::InFlight,
            ..Default::default()
        };
        let screen = current_screen(&s);
        assert!(!click_button(&screen, "送信を始める", &mut s));
        assert!(!click_button(&screen, "取り消し", &mut s));

        let mut s = BackgroundUploadState {
            view: UploadView::Waiting,
            ..Default::default()
        };
        let screen = current_screen(&s);
        assert!(!click_button(&screen, "送信を始める", &mut s));
        assert!(click_button(&screen, "取り消し", &mut s));
    }
}
