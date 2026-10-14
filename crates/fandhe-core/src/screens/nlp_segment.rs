//! Phase 11（端末機能の拡充ループ、親 #1093）型サンプル「単語分割」（Issue #1094）。
//! 入力欄 + 「分割」ボタン → OS の単語分割を呼び、件数と一覧（または失敗）を表示する 1 画面。
//! `FANDHE_PROXY_APP=nlp_segment` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod nlp_segment;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。OS 呼び出しは本モジュール外
//! （`fandhe-egui-proxy` の `device/nlp.rs`、Web は `fandhe-web-dom` の `nlp.rs`）が
//! [`take_request`] で要求を観測し [`finish_segment`] で結果を注入する。
//!
//! # 空入力を core で止めない理由
//!
//! 契約（`fandhe_device::nlp`）は「対応 OS は空入力で `Ok(vec![])`、非対応 OS は入力に
//! 関わらず `Unsupported`」。ここで止めると非対応 OS が「0 語」の成功に見えるため、
//! 空入力でも要求を立てて OS 側の判定に任せる。
//!
//! # 状態機械
//!
//! 3 OS とも同期呼び出しのため `Sending` 相は持たない（`haptics_tap` との違い）。
//!
//! ```text
//! Idle --「分割」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → finish_segment（世代一致のみ反映）--> phase=Idle + 結果表示
//! ```

use crate::{Node, Screen, WidgetId};

/// 「分割」ボタンの `WidgetId`。
const SEGMENT_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS・API 無しのときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この端末では使えません";
/// 入力が契約（長さ上限・NUL）に収まらないときの案内。上限値は SSOT
/// （`fandhe_device::nlp::MAX_INPUT_CHARS`）の複製を避けるためここへ書かない。
pub const INVALID_INPUT_STATUS: &str = "入力が長すぎるか、使えない文字を含んでいます";

/// 分割操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NlpPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側、
/// `fandhe-core` は `fandhe-device` に依存しないため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentFailure {
    /// 非対応 OS・API 無し。
    Unsupported,
    /// 入力が契約に収まらない（長さ上限・NUL）。
    InvalidInput,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentRequest {
    /// [`finish_segment`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 分割する文字列（検証・正規化は effect 側の `fandhe_device::nlp` に一本化する）。
    pub text: String,
}

/// 単語分割画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NlpSegmentState {
    /// 入力欄の値。
    pub input: String,
    /// 操作の進行状態。
    pub phase: NlpPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 直近の分割結果。件数は `words.len()`（別の値で持たない）。
    pub words: Vec<String>,
    /// 成功の案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列にする。
    pub error: String,
}

/// 入力欄の表示値（`Screen::text_field` の getter）。
pub fn get_input(state: &NlpSegmentState) -> String {
    state.input.clone()
}

/// 入力欄の入力を反映する（`Screen::text_field` の setter）。
pub fn set_input(state: &mut NlpSegmentState, value: String) {
    state.input = value;
}

/// 「分割」ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn segment(state: &mut NlpSegmentState) {
    if state.phase != NlpPhase::Idle {
        return;
    }
    state.phase = NlpPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果表示を残さない。
    state.words.clear();
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未処理の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &NlpSegmentState) -> Option<SegmentRequest> {
    (state.phase == NlpPhase::Requested).then(|| SegmentRequest {
        generation: state.generation,
        text: state.input.clone(),
    })
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_segment(
    state: &mut NlpSegmentState,
    generation: u64,
    result: Result<Vec<String>, SegmentFailure>,
) {
    if state.phase == NlpPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = NlpPhase::Idle;
    match result {
        Ok(words) => {
            state.status = format!("{} 語", words.len());
            state.words = words;
            state.error = String::new();
        }
        Err(failure) => {
            state.words.clear();
            state.status = String::new();
            state.error = match failure {
                SegmentFailure::Unsupported => UNSUPPORTED_STATUS.to_string(),
                SegmentFailure::InvalidInput => INVALID_INPUT_STATUS.to_string(),
                SegmentFailure::Failed(message) => format!("{FAILED_PREFIX}{message}"),
            };
        }
    }
}

/// 件数の案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &NlpSegmentState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &NlpSegmentState) -> String {
    state.error.clone()
}

/// 分割結果を `Node::Text` 一覧へ展開する（`.list()` の `expand`）。
pub fn word_nodes(state: &NlpSegmentState) -> Vec<Node<NlpSegmentState>> {
    state.words.iter().cloned().map(Node::Text).collect()
}

/// 単語分割画面を組み立てる。`registry.rs` の `effect` 区画から `fn(&S) -> Screen<S>` として
/// 呼ばれる契約。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &NlpSegmentState) -> Screen<NlpSegmentState> {
    let screen = Screen::new("単語分割")
        .text_field("分割する文字列", get_input, set_input)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(word_nodes);
    if state.phase != NlpPhase::Idle {
        return screen;
    }
    screen.button(SEGMENT_BUTTON_ID, "分割", segment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn requested(input: &str) -> NlpSegmentState {
        let mut state = NlpSegmentState {
            input: input.to_string(),
            ..Default::default()
        };
        segment(&mut state);
        state
    }

    #[test]
    fn segment_advances_generation_and_phase() {
        let state = requested("a b");
        assert_eq!(state.phase, NlpPhase::Requested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn segment_is_noop_while_requested() {
        let mut state = requested("a");
        segment(&mut state);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn empty_input_still_raises_request() {
        let state = requested("");
        assert_eq!(
            take_request(&state),
            Some(SegmentRequest {
                generation: 1,
                text: String::new()
            })
        );
    }

    #[test]
    fn take_request_carries_input_only_when_requested() {
        let mut state = NlpSegmentState::default();
        assert_eq!(take_request(&state), None);
        state = requested("hello world");
        assert_eq!(take_request(&state).unwrap().text, "hello world");
        finish_segment(&mut state, 1, Ok(vec![]));
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn finish_ok_count_equals_word_count() {
        let mut state = requested("hello world");
        finish_segment(&mut state, 1, Ok(vec!["hello".into(), "world".into()]));
        assert_eq!(state.phase, NlpPhase::Idle);
        assert_eq!(state.status, "2 語");
        assert_eq!(state.words.len(), 2);
        assert_eq!(state.error, "");
        assert_eq!(word_nodes(&state).len(), 2);
    }

    #[test]
    fn finish_empty_ok_is_zero_words() {
        let mut state = requested("");
        finish_segment(&mut state, 1, Ok(vec![]));
        assert_eq!(state.status, "0 語");
    }

    #[test]
    fn failures_show_error_and_no_success_text() {
        for failure in [
            SegmentFailure::Unsupported,
            SegmentFailure::InvalidInput,
            SegmentFailure::Failed("x".into()),
        ] {
            let mut state = requested("a");
            state.words = vec!["old".into()];
            finish_segment(&mut state, 1, Err(failure));
            assert_eq!(state.phase, NlpPhase::Idle);
            assert!(!state.error.is_empty());
            assert!(state.status.is_empty());
            assert!(state.words.is_empty());
        }
    }

    #[test]
    fn failed_has_prefix() {
        let mut state = requested("a");
        finish_segment(&mut state, 1, Err(SegmentFailure::Failed("x".into())));
        assert_eq!(state.error, format!("{FAILED_PREFIX}x"));
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = requested("a");
        finish_segment(&mut state, 99, Ok(vec!["a".into()]));
        assert_eq!(state.phase, NlpPhase::Requested);
        assert!(state.words.is_empty());
        let mut idle = NlpSegmentState::default();
        finish_segment(&mut idle, 0, Ok(vec!["a".into()]));
        assert!(idle.words.is_empty());
    }

    #[test]
    fn new_request_clears_previous_result() {
        let mut state = requested("a");
        finish_segment(&mut state, 1, Ok(vec!["a".into()]));
        segment(&mut state);
        assert!(state.words.is_empty());
        assert_eq!(state.status, "");
    }

    #[test]
    fn button_is_hidden_while_requested_and_shown_when_idle() {
        let mut state = NlpSegmentState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "分割", &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "分割", &mut state));
    }
}
