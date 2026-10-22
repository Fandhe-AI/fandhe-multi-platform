//! Phase 11（端末機能の拡充ループ、親 #119）型サンプル「スペルチェック」（Issue #578）。
//! 入力した短い文を OS の辞書で 1 回検査し、誤り語の位置・語・修正候補を一覧する 1 画面。
//! 辞書の無い言語は「誤り 0 件」ではなく失敗として出す。`FANDHE_PROXY_APP=spell_check` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod spell_check;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/spell_check.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_check`] で状態を進める。全対象 OS（macOS / iOS / Android /
//! Windows）で同じ UI・文言を使うため、OS 別の子 issue はこのファイルを触らない。
//!
//! ```text
//! Idle/Done/Failed --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_check（Running かつ世代一致のみ）--> Done | Failed
//! ```

use crate::{Node, Screen, WidgetId};

const ENGLISH_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const UNAVAILABLE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 入力欄の初期値（誤りを含む英文）。
pub const DEFAULT_INPUT: &str = "This sentense has a speling mistake.";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "端末の辞書で、入力した文のつづりを 1 回だけ検査します";
/// 検査を待っている間の案内。
pub const RUNNING_STATUS: &str = "検査しています…";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS・スペルチェッカーが無効のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 言語の辞書が端末に無いときの案内（この画面から辞書のダウンロードはしない）。
pub const LANGUAGE_UNAVAILABLE_MESSAGE: &str = "この言語の辞書が端末にありません";
/// 期限切れのときの案内。
pub const TIMEOUT_MESSAGE: &str = "時間内に検査が終わりませんでした";
/// 入力が検査できないときの案内。
pub const INVALID_INPUT_MESSAGE: &str = "入力が空か、検査できる長さを超えています";
/// 誤りが無かったときの案内。
pub const NO_ISSUES_MESSAGE: &str = "誤りは見つかりませんでした";
/// 誤りの一覧を打ち切ったときの案内。
pub const TRUNCATED_MESSAGE: &str = "誤りが多いため、先頭の一部だけを表示しています";
/// 修正候補が無いときの表記。
pub const NO_SUGGESTIONS: &str = "（候補なし）";

/// 検査する言語（ボタンで選ぶ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LanguageChoice {
    /// 英語（米国）。
    #[default]
    English,
    /// 辞書の無い言語（失敗系の確認用。`zz` は構文上は正しいが実在しない）。
    Unavailable,
}

impl LanguageChoice {
    /// BCP 47 の言語タグ。
    pub fn tag(&self) -> &'static str {
        match self {
            LanguageChoice::English => "en-US",
            LanguageChoice::Unavailable => "zz",
        }
    }
}

/// 誤り語 1 件の表示用データ（`fandhe_device::spell_check::SpellIssue` から effect 側が写す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellIssueView {
    /// 入力文での開始位置（文字単位、0 始まり）。
    pub start: usize,
    /// 誤り語。
    pub word: String,
    /// 修正候補。
    pub suggestions: Vec<String>,
}

/// 検査結果の表示用データ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellResultView {
    /// 実際に検査に使った言語。
    pub language: String,
    /// 誤り語（出現順）。
    pub issues: Vec<SpellIssueView>,
    /// 誤りの一覧を打ち切ったか。
    pub truncated: bool,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpellFailure {
    /// 非対応 OS・スペルチェッカーが無効。
    Unsupported,
    /// 言語の辞書が端末に無い。
    LanguageUnavailable,
    /// 期限切れ。
    Timeout,
    /// 入力不正（空・長すぎる・NUL を含む）。
    InvalidInput,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 操作の進行状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SpellPhase {
    /// 未操作。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 検査待ち。
    Running,
    /// 成功。
    Done(SpellResultView),
    /// 失敗。
    Failed(SpellFailure),
}

/// スペルチェック画面の状態。
#[derive(Debug, Clone, PartialEq)]
pub struct SpellCheckState {
    /// 入力欄の文字列。
    pub input: String,
    /// 操作の進行状態。
    pub phase: SpellPhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近に選んだ言語。
    pub language: LanguageChoice,
}

impl Default for SpellCheckState {
    fn default() -> Self {
        Self {
            input: DEFAULT_INPUT.to_string(),
            phase: SpellPhase::Idle,
            generation: 0,
            language: LanguageChoice::default(),
        }
    }
}

fn is_busy(state: &SpellCheckState) -> bool {
    matches!(state.phase, SpellPhase::Requested | SpellPhase::Running)
}

/// 入力欄の表示値（`Screen::text_field` の getter）。
pub fn get_input(state: &SpellCheckState) -> String {
    state.input.clone()
}

/// 入力欄の入力を反映する（`Screen::text_field` の setter）。
/// 処理中は検査対象の文を変えないため無視する（次の描画で入力欄は元の値に戻る）。
/// 完了済みの結果（成功・失敗）は別の文に対応して見えるため、編集した時点で消して `Idle` へ戻す。
pub fn set_input(state: &mut SpellCheckState, value: String) {
    if is_busy(state) {
        return;
    }
    if state.input == value {
        return;
    }
    state.input = value;
    if matches!(state.phase, SpellPhase::Done(_) | SpellPhase::Failed(_)) {
        state.phase = SpellPhase::Idle;
    }
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_check(state: &mut SpellCheckState, language: LanguageChoice) {
    if is_busy(state) {
        return;
    }
    state.phase = SpellPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.language = language;
}

fn request_english(state: &mut SpellCheckState) {
    request_check(state, LanguageChoice::English);
}

fn request_unavailable(state: &mut SpellCheckState) {
    request_check(state, LanguageChoice::Unavailable);
}

/// effect ドライバが未発送の要求の `(世代, 入力文, 言語タグ)` を観測する（状態は変えない）。
/// 世代は [`finish_check`] へそのまま渡す。
pub fn take_request(state: &SpellCheckState) -> Option<(u64, String, &'static str)> {
    matches!(state.phase, SpellPhase::Requested)
        .then(|| (state.generation, state.input.clone(), state.language.tag()))
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut SpellCheckState) {
    if state.phase == SpellPhase::Requested {
        state.phase = SpellPhase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み）は無視する。判定と更新はこの関数の中で完結する。
pub fn finish_check(
    state: &mut SpellCheckState,
    generation: u64,
    result: Result<SpellResultView, SpellFailure>,
) {
    if state.phase != SpellPhase::Running || state.generation != generation {
        return;
    }
    state.phase = match result {
        Ok(v) => SpellPhase::Done(v),
        Err(f) => SpellPhase::Failed(f),
    };
}

/// 処理中・結果の要約の案内文言。
pub fn status_text(state: &SpellCheckState) -> String {
    match &state.phase {
        SpellPhase::Requested | SpellPhase::Running => RUNNING_STATUS.to_string(),
        SpellPhase::Done(v) if v.issues.is_empty() => NO_ISSUES_MESSAGE.to_string(),
        SpellPhase::Done(v) if v.truncated => TRUNCATED_MESSAGE.to_string(),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &SpellCheckState) -> String {
    let SpellPhase::Failed(f) = &state.phase else {
        return String::new();
    };
    let message = match f {
        SpellFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        SpellFailure::LanguageUnavailable => LANGUAGE_UNAVAILABLE_MESSAGE,
        SpellFailure::Timeout => TIMEOUT_MESSAGE,
        SpellFailure::InvalidInput => INVALID_INPUT_MESSAGE,
        SpellFailure::Failed(m) => m,
    };
    format!("{FAILED_PREFIX}{message}")
}

fn issue_line(i: &SpellIssueView) -> String {
    let suggestions = if i.suggestions.is_empty() {
        NO_SUGGESTIONS.to_string()
    } else {
        i.suggestions.join(", ")
    };
    format!("{} 文字目: {} → {suggestions}", i.start + 1, i.word)
}

fn result_nodes(state: &SpellCheckState) -> Vec<Node<SpellCheckState>> {
    match &state.phase {
        SpellPhase::Done(v) => v.issues.iter().map(|i| Node::Text(issue_line(i))).collect(),
        _ => Vec::new(),
    }
}

fn language_line(state: &SpellCheckState) -> String {
    format!("言語: {}", state.language.tag())
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す。
pub fn current_screen(state: &SpellCheckState) -> Screen<SpellCheckState> {
    let screen = Screen::new("スペルチェック")
        .text(IDLE_HINT)
        .text_field("検査する文", get_input, set_input)
        .dynamic_text(language_line)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(result_nodes);
    if is_busy(state) {
        return screen;
    }
    screen
        .button(ENGLISH_BUTTON_ID, "英語で検査する", request_english)
        .button(
            UNAVAILABLE_BUTTON_ID,
            "辞書の無い言語で試す（zz）",
            request_unavailable,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> SpellCheckState {
        let mut s = SpellCheckState::default();
        request_check(&mut s, LanguageChoice::English);
        mark_running(&mut s);
        s
    }

    fn view(n: usize, truncated: bool) -> SpellResultView {
        SpellResultView {
            language: "en_US".into(),
            issues: (0..n)
                .map(|i| SpellIssueView {
                    start: i,
                    word: "x".into(),
                    suggestions: vec![],
                })
                .collect(),
            truncated,
        }
    }

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = SpellCheckState::default();
        request_check(&mut s, LanguageChoice::English);
        assert_eq!(
            take_request(&s),
            Some((1, DEFAULT_INPUT.to_string(), "en-US"))
        );
        request_check(&mut s, LanguageChoice::Unavailable);
        assert_eq!((s.generation, s.language), (1, LanguageChoice::English));
        mark_running(&mut s);
        request_check(&mut s, LanguageChoice::Unavailable);
        assert_eq!(s.generation, 1);
        finish_check(&mut s, 1, Ok(view(1, false)));
        assert_eq!(result_nodes(&s).len(), 1);
        request_check(&mut s, LanguageChoice::Unavailable);
        assert_eq!(s.generation, 2);
        assert!(result_nodes(&s).is_empty());
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn take_request_carries_input_and_language() {
        let mut s = SpellCheckState::default();
        set_input(&mut s, "hello wrold".into());
        request_check(&mut s, LanguageChoice::Unavailable);
        assert_eq!(take_request(&s), Some((1, "hello wrold".into(), "zz")));
        mark_running(&mut s);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn stale_and_non_running_finish_are_ignored() {
        let mut s = running();
        finish_check(&mut s, 0, Ok(view(0, false)));
        assert_eq!(s.phase, SpellPhase::Running);
        finish_check(&mut s, 1, Err(SpellFailure::Timeout));
        // 期限で確定済みの後に届いた成功は適用しない
        finish_check(&mut s, 1, Ok(view(0, false)));
        assert_eq!(s.phase, SpellPhase::Failed(SpellFailure::Timeout));
        let mut idle = SpellCheckState::default();
        finish_check(&mut idle, 0, Ok(view(0, false)));
        assert_eq!(idle.phase, SpellPhase::Idle);
    }

    #[test]
    fn edit_clears_finished_result() {
        let mut s = running();
        finish_check(&mut s, 1, Ok(view(0, false)));
        assert_eq!(status_text(&s), NO_ISSUES_MESSAGE);
        set_input(&mut s, "a typo wrold".into());
        assert_eq!(s.phase, SpellPhase::Idle);
        assert_eq!(status_text(&s), "");
        let mut f = running();
        finish_check(&mut f, 1, Err(SpellFailure::Timeout));
        set_input(&mut f, "other".into());
        assert_eq!(f.phase, SpellPhase::Idle);
    }

    #[test]
    fn same_value_edit_keeps_result() {
        let mut s = running();
        finish_check(&mut s, 1, Ok(view(1, false)));
        set_input(&mut s, DEFAULT_INPUT.into());
        assert!(matches!(s.phase, SpellPhase::Done(_)));
    }

    #[test]
    fn edit_ignored_while_busy() {
        let mut s = SpellCheckState::default();
        request_check(&mut s, LanguageChoice::English);
        set_input(&mut s, "changed".into());
        assert_eq!(s.input, DEFAULT_INPUT);
        mark_running(&mut s);
        set_input(&mut s, "changed".into());
        assert_eq!(s.input, DEFAULT_INPUT);
        assert_eq!(s.phase, SpellPhase::Running);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = SpellCheckState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, SpellPhase::Idle);
    }

    #[test]
    fn every_failure_has_nonempty_text() {
        let all = [
            SpellFailure::Unsupported,
            SpellFailure::LanguageUnavailable,
            SpellFailure::Timeout,
            SpellFailure::InvalidInput,
            SpellFailure::Failed("x".into()),
        ];
        for f in all {
            let mut s = running();
            finish_check(&mut s, 1, Err(f.clone()));
            assert!(!error_text(&s).is_empty(), "{f:?}");
            assert!(result_nodes(&s).is_empty());
        }
        let mut s = running();
        finish_check(&mut s, 1, Err(SpellFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_MESSAGE);
    }

    #[test]
    fn zero_issues_and_truncated_messages() {
        let mut s = running();
        finish_check(&mut s, 1, Ok(view(0, false)));
        assert_eq!(status_text(&s), NO_ISSUES_MESSAGE);
        let mut s = running();
        finish_check(&mut s, 1, Ok(view(2, true)));
        assert_eq!(status_text(&s), TRUNCATED_MESSAGE);
        assert_eq!(result_nodes(&s).len(), 2);
        let mut s = running();
        finish_check(&mut s, 1, Ok(view(2, false)));
        assert_eq!(status_text(&s), "");
    }

    #[test]
    fn issue_line_shows_position_and_suggestions() {
        let i = SpellIssueView {
            start: 5,
            word: "sentense".into(),
            suggestions: vec!["sentence".into(), "sentences".into()],
        };
        assert_eq!(issue_line(&i), "6 文字目: sentense → sentence, sentences");
        let none = SpellIssueView {
            suggestions: vec![],
            ..i
        };
        assert!(issue_line(&none).ends_with(NO_SUGGESTIONS));
    }

    #[test]
    fn buttons_hidden_while_busy() {
        // 固定 6 ノード（案内・入力欄・言語・状態・失敗・結果）+ ボタン 2 つ
        let count = |s: &SpellCheckState| current_screen(s).len() - 6;
        let mut s = SpellCheckState::default();
        assert_eq!(count(&s), 2);
        request_check(&mut s, LanguageChoice::English);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_check(&mut s, 1, Err(SpellFailure::Timeout));
        assert_eq!(count(&s), 2);
    }
}
