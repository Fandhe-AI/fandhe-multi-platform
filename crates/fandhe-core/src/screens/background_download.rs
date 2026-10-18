//! Phase 11 の拡充ループ（親 #100、Issue #390）型サンプル「バックグラウンドダウンロード」。
//! 「ダウンロード開始」「取り消し」の 2 ボタンと状態 1 行を持つ 1 画面。`FANDHE_PROXY_APP=background_download`
//! で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod background_download;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは `fandhe-egui-proxy` の
//! effect ドライバ（`device/background_download.rs`）が [`take_request`] で要求を観測し
//! [`finish_action`] で結果を注入する（`background_task.rs` と同型の状態機械）。全 OS 分の文言と
//! 定数はここに置き、OS ごとの子 issue はこの画面を触らない。
//!
//! 転送は OS が利用者の操作なしに進めるため、状態の SSOT は OS 側の `status()` だけとし、
//! ドライバがポーリングで [`apply_snapshot`] を呼んで [`DownloadView`] を更新する。
//! [`finish_action`] は `phase` を戻して失敗を出すだけで [`DownloadView`] は変えない
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
pub const TITLE: &str = "バックグラウンドダウンロード";
/// 説明文。
pub const DESCRIPTION: &str =
    "ファイル 1 つの転送を OS に任せます。アプリを閉じても続くかは OS によります";
/// 未開始。
pub const IDLE: &str = "まだダウンロードしていません";
/// 取り消し済み（失敗ではない）。
pub const CANCELLED: &str = "取り消しました";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗: ";
/// 状態の読み出し失敗の接頭辞（未開始と区別する）。
pub const READ_FAILED_PREFIX: &str = "状態を読み出せません: ";
/// 操作の失敗の接頭辞。
pub const ACTION_FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応環境の固定文言（proxy と web-dom の `to_message` が使う）。
pub const UNSUPPORTED: &str = "この環境ではバックグラウンドダウンロードを使えません";
/// 保存先の案内（絶対パスは出さない）。
pub const SAVE_LOCATION: &str = "保存先: アプリのキャッシュ / background-download";

/// ボタン操作の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 「ダウンロード開始」。
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

/// OS 側の転送状態の表示用写像（`fandhe_device::background_download::DownloadStatus` と対応。
/// `fandhe-core` は `fandhe-device` に依存しないので写像は proxy 側の責務）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DownloadView {
    /// まだ観測していない。
    #[default]
    Unknown,
    /// 未開始。
    Idle,
    /// 転送中。
    Running {
        /// 受信済みバイト数。
        received: u64,
        /// 総バイト数（不明なら `None`）。
        total: Option<u64>,
    },
    /// 完了。
    Completed {
        /// 保存したバイト数。
        bytes: u64,
    },
    /// 取り消し。
    Cancelled,
    /// 失敗（固定の短い文言）。
    Failed(String),
    /// 状態の読み出しに失敗した。
    ReadFailed(String),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackgroundDownloadState {
    /// ボタン操作の進行状態。
    pub phase: Phase,
    /// 直近の要求の世代。完了通知が「今の要求」のものかの照合に使う。
    pub generation: u64,
    /// OS 側の転送状態（ポーリングで更新）。
    pub view: DownloadView,
    /// 操作の失敗理由（成功時は空）。
    pub error: String,
}

/// ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// [`finish_action`] へそのまま渡す世代。
    pub generation: u64,
    /// 実行する操作。
    pub action: Action,
}

fn request(state: &mut BackgroundDownloadState, action: Action) {
    state.phase = Phase::Requested(action);
    state.generation = state.generation.wrapping_add(1);
    state.error = String::new();
}

/// 「ダウンロード開始」。処理中、または転送中は二重要求を防ぐため no-op。
pub fn start(state: &mut BackgroundDownloadState) {
    if state.phase != Phase::Idle || matches!(state.view, DownloadView::Running { .. }) {
        return;
    }
    request(state, Action::Start);
}

/// 「取り消し」。処理中は no-op。何も無いときも出す（契約上 `Ok`）。
pub fn cancel(state: &mut BackgroundDownloadState) {
    if state.phase != Phase::Idle {
        return;
    }
    request(state, Action::Cancel);
}

/// ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &BackgroundDownloadState) -> Option<Request> {
    match state.phase {
        Phase::Requested(action) => Some(Request {
            generation: state.generation,
            action,
        }),
        _ => None,
    }
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut BackgroundDownloadState) {
    if matches!(state.phase, Phase::Requested(_)) {
        state.phase = Phase::InFlight;
    }
}

/// 操作の結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は捨てる。
/// `Err` は失敗を出すだけで、転送状態（[`DownloadView`]）は変えない。
pub fn finish_action(
    state: &mut BackgroundDownloadState,
    generation: u64,
    result: Result<(), String>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.error = match result {
        Ok(()) => String::new(),
        Err(message) => format!("{ACTION_FAILED_PREFIX}{message}"),
    };
}

/// OS 側の転送状態を反映する（`phase` には触れない）。冪等。
pub fn apply_snapshot(state: &mut BackgroundDownloadState, view: DownloadView) {
    state.view = view;
}

/// 状態 1 行の文言。`total == 0` は割り算せずバイト数だけ、受信が総数を超えても 100% で止める。
pub fn format_status(view: &DownloadView) -> String {
    match view {
        DownloadView::Unknown | DownloadView::Idle => IDLE.to_string(),
        DownloadView::Running {
            received,
            total: Some(total),
        } if *total > 0 => {
            let percent = (u128::from(*received.min(total)) * 100 / u128::from(*total)).min(100);
            format!("転送中 {received} / {total} バイト（{percent}%）")
        }
        DownloadView::Running { received, .. } => format!("転送中 {received} バイト"),
        DownloadView::Completed { bytes } => format!("完了: {bytes} バイト（{SAVE_LOCATION}）"),
        DownloadView::Cancelled => CANCELLED.to_string(),
        DownloadView::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        DownloadView::ReadFailed(m) => format!("{READ_FAILED_PREFIX}{m}"),
    }
}

fn status_text(state: &BackgroundDownloadState) -> String {
    format_status(&state.view)
}

fn error_text(state: &BackgroundDownloadState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中は二重押しを防ぐためボタンを外し、転送中は開始ボタンも外す。
pub fn current_screen(state: &BackgroundDownloadState) -> Screen<BackgroundDownloadState> {
    let screen = Screen::new(TITLE)
        .text(DESCRIPTION)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != Phase::Idle {
        return screen;
    }
    let screen = if matches!(state.view, DownloadView::Running { .. }) {
        screen
    } else {
        screen.button(START_BUTTON_ID, "ダウンロード開始", start)
    };
    screen.button(CANCEL_BUTTON_ID, "取り消し", cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn in_flight(state: &mut BackgroundDownloadState, f: fn(&mut BackgroundDownloadState)) {
        f(state);
        mark_in_flight(state);
    }

    #[test]
    fn start_and_cancel_advance_generation() {
        let mut s = BackgroundDownloadState::default();
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
    fn start_is_noop_while_running_or_in_flight() {
        let mut s = BackgroundDownloadState {
            view: DownloadView::Running {
                received: 1,
                total: None,
            },
            ..Default::default()
        };
        start(&mut s);
        assert_eq!(s.phase, Phase::Idle);
        let mut s = BackgroundDownloadState::default();
        in_flight(&mut s, start);
        start(&mut s);
        cancel(&mut s);
        assert_eq!((s.phase, s.generation), (Phase::InFlight, 1));
    }

    #[test]
    fn stale_generation_and_idle_completions_are_dropped() {
        let mut s = BackgroundDownloadState::default();
        in_flight(&mut s, start);
        finish_action(&mut s, 7, Err("x".into()));
        assert_eq!(s.phase, Phase::InFlight);
        finish_action(&mut s, 1, Ok(()));
        finish_action(&mut s, 1, Err("遅れて届いた".into()));
        assert_eq!(s.error, "");
    }

    #[test]
    fn err_completion_shows_error_and_keeps_view() {
        let mut s = BackgroundDownloadState::default();
        in_flight(&mut s, start);
        finish_action(&mut s, 1, Err(UNSUPPORTED.into()));
        assert!(s.error.starts_with(ACTION_FAILED_PREFIX));
        assert_eq!(s.view, DownloadView::Unknown);
        assert_eq!(format_status(&s.view), IDLE);
    }

    #[test]
    fn format_status_arms() {
        let r = |received, total| format_status(&DownloadView::Running { received, total });
        assert_eq!(r(20, Some(40)), "転送中 20 / 40 バイト（50%）");
        assert_eq!(r(20, None), "転送中 20 バイト");
        assert_eq!(r(20, Some(0)), "転送中 20 バイト");
        assert_eq!(r(50, Some(40)), "転送中 50 / 40 バイト（100%）");
        assert!(r(u64::MAX, Some(u64::MAX)).ends_with("（100%）"));
        assert!(
            format_status(&DownloadView::Completed { bytes: 40 }).starts_with("完了: 40 バイト")
        );
        assert_eq!(format_status(&DownloadView::Cancelled), CANCELLED);
        assert_eq!(
            format_status(&DownloadView::Failed("HTTP 404".into())),
            "失敗: HTTP 404"
        );
        assert!(
            format_status(&DownloadView::ReadFailed("x".into())).starts_with(READ_FAILED_PREFIX)
        );
    }

    #[test]
    fn buttons_follow_state() {
        let mut s = BackgroundDownloadState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, "ダウンロード開始", &mut s));
        assert_eq!(s.phase, Phase::Requested(Action::Start));

        let mut s = BackgroundDownloadState {
            phase: Phase::InFlight,
            ..Default::default()
        };
        let screen = current_screen(&s);
        assert!(!click_button(&screen, "ダウンロード開始", &mut s));
        assert!(!click_button(&screen, "取り消し", &mut s));

        let mut s = BackgroundDownloadState {
            view: DownloadView::Running {
                received: 1,
                total: None,
            },
            ..Default::default()
        };
        let screen = current_screen(&s);
        assert!(!click_button(&screen, "ダウンロード開始", &mut s));
        assert!(click_button(&screen, "取り消し", &mut s));
    }
}
