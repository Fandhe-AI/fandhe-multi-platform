//! Phase 11（端末機能の拡充ループ、親 #94）型サンプル「印刷」（Issue #325）。
//! 固定の短い文章を OS の印刷ダイアログへ渡す 1 ボタンだけの 1 画面。
//! `FANDHE_PROXY_APP=print_page` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod print_page;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。入力の検査と OS 呼び出しは `fandhe-egui-proxy` の effect ドライバ
//! （`device/print.rs`）が [`take_request`] で要求を観測して行い、[`finish_print`] で結果を
//! 注入する。本画面は二重押し防止と文言だけを持つ。全 OS 共通の画面で、OS 固有の操作は無い。
//!
//! # 結果を混同しない理由
//!
//! 「OS の印刷キューへ渡した」「取り消した」「ダイアログを開く依頼だけで結果は不明」を黙って
//! 「印刷しました」に丸めると、利用者は紙に出たと誤解する。どの文言も印刷完了を言わない。
//! OS が判定した正常な結果は `status`、入力不正・失敗は `error`（`TextRole::Error`）へ書き分ける。
//! 世代番号（`generation`）で古い完了や `Idle` で届いた完了を無視する（`compose_mail_sms` と同じ判断）。

use crate::{Screen, WidgetId};

/// 固定の見出し（日本語と ASCII の混在。印刷結果と見比べられるよう画面にも出す）。
pub const PRINT_TITLE: &str = "fandhe 印刷テスト / Print test";
/// 固定の本文（3〜5 行、各行 80 文字以内、末尾改行なし）。
pub const PRINT_BODY: &str = "これは fandhe の印刷テストです。\nThis page is printed from fandhe.\n日本語と ASCII を混ぜた 1 ページの文章です。\nLine 4: 0123456789 ABCDEFG";

/// OS の印刷系へジョブを渡したときの案内（紙に出たとは言わない）。
pub const SUBMITTED_STATUS: &str =
    "印刷を依頼しました（OS の印刷キューへ渡しました。用紙への出力は OS 側で確認してください）";
/// 利用者が取り消したときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "印刷を取り消しました";
/// 結果を OS が返さないときの案内。
pub const UNCONFIRMED_STATUS: &str =
    "印刷ダイアログを開くよう依頼しました（印刷したかは OS が返しません）";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では印刷できません";
/// 入力不正の接頭辞。
pub const INVALID_PREFIX: &str = "入力を確認してください: ";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 画面に出す補足（プリンタが無い環境での確かめ方）。
pub const NOTE_TEXT: &str =
    "固定の短い文章を 1 ページ印刷します。プリンタが無ければ OS の PDF 出力で確かめてください";
/// 「印刷する」ボタンの `WidgetId`。
pub const PRINT_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrintPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_printing`] 済み）。
    Printing,
}

/// OS が判定した結果（`PrintOutcome` に非対応を加えたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 印刷系へジョブを渡した。
    Submitted,
    /// 取り消し。
    Cancelled,
    /// 結果を OS が返さない。
    Unconfirmed,
    /// この環境では非対応。
    Unsupported,
}

/// 失敗（画面には固定文言だけを出す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrintFailure {
    /// 入力不正（OS へは渡していない）。
    Invalid(String),
    /// 内部失敗。
    Failed(String),
}

/// effect ドライバへ渡す要求。値は未検査で、検査は proxy 側（`PrintRequest`）が行う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintRequestView {
    /// [`finish_print`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 見出し。
    pub title: String,
    /// 本文（`\n` 区切り）。
    pub body: String,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrintPageState {
    /// 進行状態。
    pub phase: PrintPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 入力不正・失敗（`TextRole::Error`）。
    pub error: String,
}

/// 「印刷する」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn print(state: &mut PrintPageState) {
    if state.phase != PrintPhase::Idle {
        return;
    }
    state.phase = PrintPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &PrintPageState) -> Option<PrintRequestView> {
    (state.phase == PrintPhase::Requested).then(|| PrintRequestView {
        generation: state.generation,
        title: PRINT_TITLE.to_string(),
        body: PRINT_BODY.to_string(),
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_printing(state: &mut PrintPageState) {
    if state.phase == PrintPhase::Requested {
        state.phase = PrintPhase::Printing;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_print(
    state: &mut PrintPageState,
    generation: u64,
    result: Result<CoreOutcome, PrintFailure>,
) {
    if state.phase == PrintPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PrintPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::Submitted) => state.status = SUBMITTED_STATUS.to_string(),
        Ok(CoreOutcome::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Ok(CoreOutcome::Unconfirmed) => state.status = UNCONFIRMED_STATUS.to_string(),
        Ok(CoreOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(PrintFailure::Invalid(m)) => state.error = format!("{INVALID_PREFIX}{m}"),
        Err(PrintFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &PrintPageState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &PrintPageState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。見出しと本文をそのまま表示し、印刷結果と見比べられるようにする。
/// 処理中はボタンを外し、同時に 2 件出さない。
pub fn current_screen(state: &PrintPageState) -> Screen<PrintPageState> {
    let mut screen = Screen::new("印刷").text(NOTE_TEXT).text(PRINT_TITLE);
    for line in PRINT_BODY.split('\n') {
        screen = screen.text(line);
    }
    let screen = screen.dynamic_text(status_text).error_text(error_text);
    if state.phase != PrintPhase::Idle {
        return screen;
    }
    screen.button(PRINT_BUTTON_ID, "印刷する", print)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printing() -> PrintPageState {
        let mut s = PrintPageState::default();
        print(&mut s);
        mark_printing(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, PrintFailure>) -> (String, String) {
        let mut s = printing();
        let g = s.generation;
        finish_print(&mut s, g, r);
        assert_eq!(s.phase, PrintPhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn results_are_distinct_and_none_claims_printed() {
        let all = [
            shown(Ok(CoreOutcome::Submitted)),
            shown(Ok(CoreOutcome::Cancelled)),
            shown(Ok(CoreOutcome::Unconfirmed)),
            shown(Ok(CoreOutcome::Unsupported)),
            shown(Err(PrintFailure::Invalid("x".into()))),
            shown(Err(PrintFailure::Failed("x".into()))),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
            let text = format!("{}{}", a.0, a.1);
            assert!(!text.contains("印刷しました") && !text.contains("印刷済み"));
        }
    }

    #[test]
    fn non_failures_go_to_status_and_failures_to_error() {
        for o in [
            CoreOutcome::Submitted,
            CoreOutcome::Cancelled,
            CoreOutcome::Unconfirmed,
            CoreOutcome::Unsupported,
        ] {
            let (status, error) = shown(Ok(o));
            assert!(!status.is_empty() && error.is_empty());
        }
        let (status, error) = shown(Err(PrintFailure::Failed("x".into())));
        assert!(status.is_empty() && error.starts_with(FAILED_PREFIX));
        let (status, error) = shown(Err(PrintFailure::Invalid("x".into())));
        assert!(status.is_empty() && error.starts_with(INVALID_PREFIX));
    }

    #[test]
    fn double_press_is_noop_while_in_flight() {
        let mut s = PrintPageState::default();
        assert!(take_request(&s).is_none());
        print(&mut s);
        let g = s.generation;
        print(&mut s);
        assert_eq!(s.generation, g);
        mark_printing(&mut s);
        assert!(take_request(&s).is_none());
        print(&mut s);
        assert_eq!((s.phase, s.generation), (PrintPhase::Printing, g));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = printing();
        let g = s.generation + 1;
        finish_print(&mut s, g, Ok(CoreOutcome::Submitted));
        assert_eq!(s.phase, PrintPhase::Printing);
        assert!(s.status.is_empty());

        let mut idle = PrintPageState::default();
        finish_print(&mut idle, 0, Ok(CoreOutcome::Submitted));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn buttons_hidden_while_busy() {
        let idle = current_screen(&PrintPageState::default());
        let busy = current_screen(&printing());
        assert!(format!("{idle:?}").contains("印刷する"));
        assert!(!format!("{busy:?}").contains("印刷する"));
    }

    #[test]
    fn fixed_values_fit_the_print_request_limits() {
        // 実検証は proxy 側のテストで `PrintRequest::new` を通す。ここでは制約を自前で確認する。
        assert!((1..=64).contains(&PRINT_TITLE.chars().count()));
        let lines: Vec<&str> = PRINT_BODY.split('\n').collect();
        assert!((3..=5).contains(&lines.len()));
        for l in lines {
            assert!((1..=80).contains(&l.chars().count()));
        }
    }
}
