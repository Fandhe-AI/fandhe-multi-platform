//! Phase 11（端末機能の拡充ループ、親 #115）型サンプル「端末内翻訳」（Issue #535）。
//! 固定の日本語の原文を、選んだ言語へ端末内で 1 回訳して表示する 1 画面。使えない端末・言語の
//! 組み合わせでは理由つきの失敗を出す。`FANDHE_PROXY_APP=translation` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod translation;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/translation.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_translate`] で状態を進める。全対象 OS（macOS / iOS /
//! Android / Web）で同じ UI・文言を使うため、OS 別の子 issue はこのファイルを触らない。Web でだけ
//! 起きる失敗種別（[`TranslateFailure::PermissionDenied`]・[`TranslateFailure::Interrupted`]）の
//! 文言もここで定義する。
//!
//! ```text
//! Idle/Done/Failed --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_translate（Running かつ世代一致のみ）--> Done | Failed
//! ```

use crate::{Node, Screen, WidgetId};

const ENGLISH_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const UNSUPPORTED_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 訳す固定の原文（利用者の入力は受けない）。
pub const SOURCE_TEXT: &str = "今日は良い天気です。";
/// 原文の言語（BCP 47。自動判定はしない）。
pub const SOURCE_LANG: &str = "ja";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "端末内の翻訳で、次の文を 1 回だけ訳して表示します";
/// 翻訳を待っている間の案内。
pub const RUNNING_STATUS: &str = "翻訳しています…";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 言語の組み合わせが非対応のときの案内。
pub const LANGUAGE_UNSUPPORTED_MESSAGE: &str = "この言語の組み合わせは翻訳できません";
/// 言語データが端末に無いときの案内（この画面からダウンロードは始めない）。
pub const NOT_INSTALLED_MESSAGE: &str =
    "翻訳用の言語データがまだ端末にありません（この画面からはダウンロードしません）";
/// 期限切れのときの案内。
pub const TIMEOUT_MESSAGE: &str = "時間内に翻訳が終わりませんでした";
/// 空の訳文が返ったときの案内。
pub const EMPTY_MESSAGE: &str = "空の訳文が返りました";
/// ブラウザが利用を許可しなかったときの案内（Web 用）。
pub const PERMISSION_DENIED_MESSAGE: &str = "ブラウザが利用を許可しませんでした";
/// 画面を離れて中断したときの案内（Web の `pagehide` 用）。
pub const INTERRUPTED_MESSAGE: &str = "画面を離れたため中断しました";

/// 訳先の言語（ボタンで選ぶ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetLang {
    /// 英語。
    #[default]
    English,
    /// 非対応の言語（失敗系の確認用。`zz` は構文上は正しいが実在しない）。
    Unsupported,
}

impl TargetLang {
    /// BCP 47 の言語タグ。
    pub fn tag(&self) -> &'static str {
        match self {
            TargetLang::English => "en",
            TargetLang::Unsupported => "zz",
        }
    }
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslateFailure {
    /// 非対応 OS・OS が古い。
    Unsupported,
    /// 言語の組み合わせが非対応。
    LanguageUnsupported,
    /// 言語データが端末に無い（未導入・取得中）。
    NotInstalled,
    /// 期限切れ。
    Timeout,
    /// 空の訳文。
    Empty,
    /// ブラウザが利用を許可しなかった（Web 用）。
    PermissionDenied,
    /// 画面を離れて中断した（Web の `pagehide` 用）。
    Interrupted,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 操作の進行状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum TranslatePhase {
    /// 未操作。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 翻訳待ち。
    Running,
    /// 成功（訳文）。
    Done {
        /// 訳文の全文。
        text: String,
    },
    /// 失敗。
    Failed(TranslateFailure),
}

/// 端末内翻訳画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TranslationState {
    /// 操作の進行状態。
    pub phase: TranslatePhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近に選んだ訳先の言語。
    pub target: TargetLang,
}

fn is_busy(state: &TranslationState) -> bool {
    matches!(
        state.phase,
        TranslatePhase::Requested | TranslatePhase::Running
    )
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_translate(state: &mut TranslationState, target: TargetLang) {
    if is_busy(state) {
        return;
    }
    state.phase = TranslatePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.target = target;
}

fn request_english(state: &mut TranslationState) {
    request_translate(state, TargetLang::English);
}

fn request_unsupported(state: &mut TranslationState) {
    request_translate(state, TargetLang::Unsupported);
}

/// effect ドライバが未発送の要求の `(世代, 訳先タグ)` を観測する（状態は変えない）。世代は
/// [`finish_translate`] へそのまま渡す。
pub fn take_request(state: &TranslationState) -> Option<(u64, &'static str)> {
    matches!(state.phase, TranslatePhase::Requested)
        .then_some((state.generation, state.target.tag()))
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut TranslationState) {
    if state.phase == TranslatePhase::Requested {
        state.phase = TranslatePhase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み）は無視する。判定と更新はこの関数の中で完結する。
pub fn finish_translate(
    state: &mut TranslationState,
    generation: u64,
    result: Result<String, TranslateFailure>,
) {
    if state.phase != TranslatePhase::Running || state.generation != generation {
        return;
    }
    state.phase = match result {
        Ok(text) => TranslatePhase::Done { text },
        Err(f) => TranslatePhase::Failed(f),
    };
}

/// 処理中の案内文言。
pub fn status_text(state: &TranslationState) -> String {
    if is_busy(state) {
        RUNNING_STATUS.to_string()
    } else {
        String::new()
    }
}

/// 成功時の訳文。
pub fn result_text(state: &TranslationState) -> String {
    match &state.phase {
        TranslatePhase::Done { text } => text.clone(),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &TranslationState) -> String {
    let TranslatePhase::Failed(f) = &state.phase else {
        return String::new();
    };
    let message = match f {
        TranslateFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        TranslateFailure::LanguageUnsupported => LANGUAGE_UNSUPPORTED_MESSAGE,
        TranslateFailure::NotInstalled => NOT_INSTALLED_MESSAGE,
        TranslateFailure::Timeout => TIMEOUT_MESSAGE,
        TranslateFailure::Empty => EMPTY_MESSAGE,
        TranslateFailure::PermissionDenied => PERMISSION_DENIED_MESSAGE,
        TranslateFailure::Interrupted => INTERRUPTED_MESSAGE,
        TranslateFailure::Failed(m) => m,
    };
    format!("{FAILED_PREFIX}{message}")
}

fn source_line(state: &TranslationState) -> String {
    format!(
        "原文: {SOURCE_TEXT}（{SOURCE_LANG} → {}）",
        state.target.tag()
    )
}

fn result_nodes(state: &TranslationState) -> Vec<Node<TranslationState>> {
    let t = result_text(state);
    if t.is_empty() {
        Vec::new()
    } else {
        vec![Node::Text(t)]
    }
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す。
pub fn current_screen(state: &TranslationState) -> Screen<TranslationState> {
    let screen = Screen::new("端末内翻訳")
        .text(IDLE_HINT)
        .dynamic_text(source_line)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(result_nodes);
    if is_busy(state) {
        return screen;
    }
    screen
        .button(ENGLISH_BUTTON_ID, "英語に訳す", request_english)
        .button(
            UNSUPPORTED_BUTTON_ID,
            "非対応の言語で試す（ja → zz）",
            request_unsupported,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> TranslationState {
        let mut s = TranslationState::default();
        request_translate(&mut s, TargetLang::English);
        mark_running(&mut s);
        s
    }

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = TranslationState::default();
        request_translate(&mut s, TargetLang::English);
        assert_eq!(take_request(&s), Some((1, "en")));
        request_translate(&mut s, TargetLang::Unsupported);
        assert_eq!((s.generation, s.target), (1, TargetLang::English));
        mark_running(&mut s);
        request_translate(&mut s, TargetLang::Unsupported);
        assert_eq!(s.generation, 1);
        finish_translate(&mut s, 1, Ok("Hi".into()));
        assert_eq!(result_text(&s), "Hi");
        request_translate(&mut s, TargetLang::Unsupported);
        assert_eq!(s.generation, 2);
        assert_eq!(result_text(&s), "");
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn take_request_returns_chosen_language() {
        let mut s = TranslationState::default();
        request_translate(&mut s, TargetLang::Unsupported);
        assert_eq!(take_request(&s), Some((1, "zz")));
        mark_running(&mut s);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn stale_and_non_running_finish_are_ignored() {
        let mut s = running();
        finish_translate(&mut s, 0, Ok("x".into()));
        assert_eq!(s.phase, TranslatePhase::Running);
        finish_translate(&mut s, 1, Err(TranslateFailure::Timeout));
        // 期限で確定済みの後に届いた成功は適用しない
        finish_translate(&mut s, 1, Ok("x".into()));
        assert_eq!(s.phase, TranslatePhase::Failed(TranslateFailure::Timeout));
        let mut idle = TranslationState::default();
        finish_translate(&mut idle, 0, Ok("x".into()));
        assert_eq!(idle.phase, TranslatePhase::Idle);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = TranslationState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, TranslatePhase::Idle);
    }

    #[test]
    fn every_failure_has_nonempty_text() {
        let all = [
            TranslateFailure::Unsupported,
            TranslateFailure::LanguageUnsupported,
            TranslateFailure::NotInstalled,
            TranslateFailure::Timeout,
            TranslateFailure::Empty,
            TranslateFailure::PermissionDenied,
            TranslateFailure::Interrupted,
            TranslateFailure::Failed("x".into()),
        ];
        for f in all {
            let mut s = running();
            finish_translate(&mut s, 1, Err(f.clone()));
            assert!(!error_text(&s).is_empty(), "{f:?}");
            assert_eq!(result_text(&s), "");
        }
        let mut s = running();
        finish_translate(&mut s, 1, Err(TranslateFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_MESSAGE);
    }

    #[test]
    fn buttons_hidden_while_busy() {
        // 固定 5 ノード（案内・原文・状態・失敗・結果）+ ボタン 2 つ
        let count = |s: &TranslationState| current_screen(s).len() - 5;
        let mut s = TranslationState::default();
        assert_eq!(count(&s), 2);
        request_translate(&mut s, TargetLang::English);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_translate(&mut s, 1, Err(TranslateFailure::Timeout));
        assert_eq!(count(&s), 2);
    }
}
