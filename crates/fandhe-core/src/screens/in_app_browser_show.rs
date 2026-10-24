//! Phase 11（端末機能の拡充ループ、親 #135）型サンプル「アプリ内ブラウザ」（Issue #701）。
//! 固定の公開 URL を OS のアプリ内ブラウザ（iOS の `SFSafariViewController`、Android の
//! Custom Tabs）で 1 回開き、閉じて戻ったことを表示する 1 画面。
//! `FANDHE_PROXY_APP=in_app_browser_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod in_app_browser_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/in_app_browser.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_open`] で状態を進める。全対象 OS の分の状態・文言は
//! ここで定義するため、OS 別の子 issue（#702・#703）はこのファイルを触らない。
//!
//! 開く URL は [`TARGET_URL`] の固定値で、入力欄は置かない。開いた後に閉じる取り消しも置かない
//! （守らない範囲）。「閉じた」の文言は読み込みの成功を言わない。OS のエラー文は表示しない。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_open（Running かつ世代一致のみ）--> Idle（result に Ok(()) か Err(失敗)）
//! ```

use crate::{Screen, WidgetId};

const OPEN_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 開く固定の公開 URL。
pub const TARGET_URL: &str = "https://example.com/";

/// 未操作時の案内。
pub const IDLE_HINT: &str = "固定の公開ページ（https://example.com/）をアプリ内ブラウザで開きます（対応は iOS・Android。macOS・Windows では使えません）";
/// 開いている間の案内。
pub const RUNNING_STATUS: &str = "アプリ内ブラウザを開いています。閉じるとこの画面に戻ります";
/// 閉じて戻ったときの文言（ページを読み込めたかは言わない）。
pub const CLOSED_STATUS: &str =
    "アプリ内ブラウザを閉じました（ページを読み込めたかは分かりません）";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "開けませんでした: ";
/// 非対応 OS のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この環境ではアプリ内ブラウザを使えません";
/// URL が不正だったときの案内。
pub const INVALID_URL_MESSAGE: &str = "開く URL が不正でした";
/// 表示先がないときの案内。
pub const NO_PRESENTER_MESSAGE: &str = "表示できる画面がありませんでした";
/// 開けるブラウザがないときの案内。
pub const NO_BROWSER_MESSAGE: &str = "開けるブラウザがありませんでした";
/// 期限切れのときの案内。
pub const TIMED_OUT_MESSAGE: &str = "時間内に開けませんでした";
/// 表示前に中断されたときの案内。
pub const INTERRUPTED_MESSAGE: &str = "表示の前に中断されました";
/// それ以外の失敗の案内。
pub const INTERNAL_MESSAGE: &str = "原因不明のエラーが起きました";

/// 開けなかった理由（`fandhe_device::in_app_browser::InAppBrowserFailure` から写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFailure {
    /// 非対応 OS。
    Unsupported,
    /// URL が不正。
    InvalidUrl,
    /// 表示先がない。
    NoPresenter,
    /// 開けるブラウザがない。
    NoBrowser,
    /// 開くまでの期限切れ。
    TimedOut,
    /// 表示前の中断。
    Interrupted,
    /// それ以外。
    Internal,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作、または完了後。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 開いている（利用者が閉じるまで）。
    Running,
}

/// アプリ内ブラウザ画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InAppBrowserShowState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の結果。`Ok(())` は「閉じた」、`Err` は開けなかった理由。未完了なら `None`。
    pub result: Option<Result<(), CoreFailure>>,
}

fn is_busy(state: &InAppBrowserShowState) -> bool {
    state.phase != Phase::Idle
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_open(state: &mut InAppBrowserShowState) {
    if is_busy(state) {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.result = None;
}

/// effect ドライバが未発送の要求の世代と開く URL を観測する（状態は変えない）。世代は
/// [`finish_open`] へそのまま渡す。
pub fn take_request(state: &InAppBrowserShowState) -> Option<(u64, &'static str)> {
    (state.phase == Phase::Requested).then_some((state.generation, TARGET_URL))
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut InAppBrowserShowState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み・未要求）は無視する。反映したかどうかを返す。
pub fn finish_open(
    state: &mut InAppBrowserShowState,
    generation: u64,
    result: Result<(), CoreFailure>,
) -> bool {
    if state.phase != Phase::Running || state.generation != generation {
        return false;
    }
    state.phase = Phase::Idle;
    state.result = Some(result);
    true
}

/// 状態の案内文言（処理中、または「閉じた」）。
pub fn status_text(state: &InAppBrowserShowState) -> String {
    if is_busy(state) {
        return RUNNING_STATUS.to_string();
    }
    match state.result {
        Some(Ok(())) => CLOSED_STATUS.to_string(),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &InAppBrowserShowState) -> String {
    let Some(Err(f)) = state.result else {
        return String::new();
    };
    let message = match f {
        CoreFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        CoreFailure::InvalidUrl => INVALID_URL_MESSAGE,
        CoreFailure::NoPresenter => NO_PRESENTER_MESSAGE,
        CoreFailure::NoBrowser => NO_BROWSER_MESSAGE,
        CoreFailure::TimedOut => TIMED_OUT_MESSAGE,
        CoreFailure::Interrupted => INTERRUPTED_MESSAGE,
        CoreFailure::Internal => INTERNAL_MESSAGE,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す（取り消しボタンは置かない）。
pub fn current_screen(state: &InAppBrowserShowState) -> Screen<InAppBrowserShowState> {
    let screen = Screen::new("アプリ内ブラウザ")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if is_busy(state) {
        return screen;
    }
    screen.button(OPEN_BUTTON_ID, "アプリ内ブラウザで開く", request_open)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> InAppBrowserShowState {
        let mut s = InAppBrowserShowState::default();
        request_open(&mut s);
        mark_running(&mut s);
        s
    }

    const ALL: [CoreFailure; 7] = [
        CoreFailure::Unsupported,
        CoreFailure::InvalidUrl,
        CoreFailure::NoPresenter,
        CoreFailure::NoBrowser,
        CoreFailure::TimedOut,
        CoreFailure::Interrupted,
        CoreFailure::Internal,
    ];

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = InAppBrowserShowState::default();
        request_open(&mut s);
        assert_eq!(take_request(&s), Some((1, TARGET_URL)));
        request_open(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        assert_eq!(take_request(&s), None);
        request_open(&mut s);
        assert_eq!(s.generation, 1);
        assert!(finish_open(&mut s, 1, Ok(())));
        assert_eq!(s.phase, Phase::Idle);
        request_open(&mut s);
        assert_eq!(s.generation, 2);
        assert_eq!(s.result, None);
        assert_eq!(status_text(&InAppBrowserShowState::default()), "");
    }

    #[test]
    fn stale_and_idle_finish_are_ignored() {
        let mut s = running();
        assert!(!finish_open(&mut s, 0, Ok(())));
        assert_eq!(s.phase, Phase::Running);
        assert!(finish_open(&mut s, 1, Err(CoreFailure::TimedOut)));
        // 期限で確定済みの後に届いた「閉じた」は適用しない
        assert!(!finish_open(&mut s, 1, Ok(())));
        assert_eq!(s.result, Some(Err(CoreFailure::TimedOut)));
        let mut idle = InAppBrowserShowState::default();
        assert!(!finish_open(&mut idle, 0, Ok(())));
        assert_eq!(idle.result, None);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = InAppBrowserShowState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn every_failure_shows_error_and_no_success_text() {
        for f in ALL {
            let mut s = running();
            finish_open(&mut s, 1, Err(f));
            let t = error_text(&s);
            assert!(!t.is_empty(), "{f:?}");
            assert_eq!(status_text(&s), "", "{f:?}");
            if f != CoreFailure::Unsupported {
                assert!(t.starts_with(FAILED_PREFIX), "{t}");
            }
        }
    }

    #[test]
    fn closed_text_does_not_claim_page_loaded() {
        let mut s = running();
        finish_open(&mut s, 1, Ok(()));
        let t = status_text(&s);
        assert_eq!(t, CLOSED_STATUS);
        assert_eq!(error_text(&s), "");
        for word in ["成功", "読み込みました", "表示されました"] {
            assert!(!t.contains(word), "{word}");
        }
        assert!(t.contains("分かりません"));
    }

    #[test]
    fn unsupported_is_not_shown_as_success() {
        let mut s = running();
        finish_open(&mut s, 1, Err(CoreFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_MESSAGE);
        assert_eq!(status_text(&s), "");
    }

    #[test]
    fn target_url_is_https() {
        assert!(TARGET_URL.starts_with("https://"));
    }

    #[test]
    fn button_hidden_while_busy() {
        // 固定 3 ノード（案内・状態・失敗）+ ボタン 1 つ
        let count = |s: &InAppBrowserShowState| current_screen(s).len() - 3;
        let mut s = InAppBrowserShowState::default();
        assert_eq!(count(&s), 1);
        request_open(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_open(&mut s, 1, Err(CoreFailure::Interrupted));
        assert_eq!(count(&s), 1);
    }
}
