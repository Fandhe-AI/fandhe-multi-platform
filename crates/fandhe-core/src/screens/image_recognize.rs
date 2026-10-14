//! Phase 11（端末機能の拡充ループ、親 #1120）型サンプル「画像の文字認識」（Issue #1250）。
//! 「写真を選んで文字を読む」ボタン → OS の画像選択 → 選んだ 1 枚の文字を OCR して
//! 行の一覧を表示する 1 画面。`FANDHE_PROXY_APP=image_recognize` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod image_recognize;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは本モジュール外
//! （`fandhe-egui-proxy` の `device/image_recognition.rs`）が [`take_request`] で要求を観測し、
//! [`mark_recognizing`]・[`finish_recognize`] で状態を進める。全対象 OS（macOS / Windows /
//! iOS / Android）で同じ UI・文言を使うため、OS 別の子 issue はこのファイルを触らない。
//!
//! # 成功・キャンセル・失敗を混同しない
//!
//! [`finish_recognize`] の `Ok(Some)` は成功（空の一覧は「文字なし」の成功）、`Ok(None)` は
//! 写真選択のキャンセル、`Err` は失敗。成功・キャンセルは `status`（通常色）、失敗は
//! `error`（`TextRole::Error`）へ書き分け、失敗・キャンセルでは `lines` を必ず空に戻す。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1)
//!   --effect--> Recognizing (写真選択 → OCR)
//!   --finish_recognize（世代一致のみ）--> Idle + 結果表示
//! ```

use crate::{Node, Screen, WidgetId};

/// ボタンの `WidgetId`。
const RECOGNIZE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 未操作時の案内。
pub const IDLE_HINT: &str = "「写真を選んで文字を読む」で画像を 1 枚選びます";
/// 選択・認識の最中の案内。
pub const RECOGNIZING_STATUS: &str = "読み取っています…";
/// 写真選択がキャンセルされたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "選択しませんでした";
/// 文字が 1 行も見つからなかったときの案内（成功。失敗色にしない）。
pub const NO_TEXT_STATUS: &str = "文字は見つかりませんでした";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この端末では使えません";
/// 認識言語の データが端末に無いときの案内。
pub const LANGUAGE_UNAVAILABLE_STATUS: &str = "この端末では文字認識の言語データを使えません";
/// 画像の辺が上限を超えるときの案内。
pub const INVALID_INPUT_STATUS: &str = "画像が大きすぎます";

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageRecognizePhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 写真選択または OCR の最中。
    Recognizing,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側。
/// `fandhe-core` は `fandhe-device` に依存しないため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecognizeFailure {
    /// 非対応 OS。
    Unsupported,
    /// 認識言語のデータが無い。
    LanguageUnavailable,
    /// 画像の辺が上限を超える。
    InvalidInput,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// 画像の文字認識画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageRecognizeState {
    /// 操作の進行状態。
    pub phase: ImageRecognizePhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 認識した行。件数は `lines.len()`。
    pub lines: Vec<String>,
    /// 成功・キャンセル・処理中の案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error`）。
    pub error: String,
}

/// ボタンのハンドラ。`Idle` 以外は no-op。前回の結果は新しい要求の時点で消す。
pub fn request_recognize(state: &mut ImageRecognizeState) {
    if state.phase != ImageRecognizePhase::Idle {
        return;
    }
    state.phase = ImageRecognizePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.lines.clear();
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。戻り値の世代番号は
/// [`finish_recognize`] へそのまま渡す契約。
pub fn take_request(state: &ImageRecognizeState) -> Option<u64> {
    (state.phase == ImageRecognizePhase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_recognizing(state: &mut ImageRecognizeState) {
    if state.phase == ImageRecognizePhase::Requested {
        state.phase = ImageRecognizePhase::Recognizing;
        state.status = RECOGNIZING_STATUS.to_string();
    }
}

/// 結果を反映する。`Ok(Some)` は認識結果（空は文字なしの成功）、`Ok(None)` はキャンセル、
/// `Err` は失敗。`Idle` のとき・世代が一致しないときは無視する。
pub fn finish_recognize(
    state: &mut ImageRecognizeState,
    generation: u64,
    result: Result<Option<Vec<String>>, RecognizeFailure>,
) {
    if state.phase == ImageRecognizePhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ImageRecognizePhase::Idle;
    state.error = String::new();
    match result {
        Ok(Some(lines)) => {
            state.status = if lines.is_empty() {
                NO_TEXT_STATUS.to_string()
            } else {
                format!("{} 行", lines.len())
            };
            state.lines = lines;
        }
        Ok(None) => {
            state.status = CANCELLED_STATUS.to_string();
            state.lines.clear();
        }
        Err(failure) => {
            state.status = String::new();
            state.lines.clear();
            state.error = match failure {
                RecognizeFailure::Unsupported => UNSUPPORTED_STATUS.to_string(),
                RecognizeFailure::LanguageUnavailable => LANGUAGE_UNAVAILABLE_STATUS.to_string(),
                RecognizeFailure::InvalidInput => INVALID_INPUT_STATUS.to_string(),
                RecognizeFailure::Failed(message) => format!("{FAILED_PREFIX}{message}"),
            };
        }
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &ImageRecognizeState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &ImageRecognizeState) -> String {
    state.error.clone()
}

/// 認識結果を `Node::Text` 一覧へ展開する（`.list()` の `expand`）。
pub fn line_nodes(state: &ImageRecognizeState) -> Vec<Node<ImageRecognizeState>> {
    state.lines.iter().cloned().map(Node::Text).collect()
}

/// 画像の文字認識画面を組み立てる。ボタンは `Idle` のときだけ出す。
pub fn current_screen(state: &ImageRecognizeState) -> Screen<ImageRecognizeState> {
    let screen = Screen::new("画像の文字認識")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(line_nodes);
    if state.phase != ImageRecognizePhase::Idle {
        return screen;
    }
    screen.button(
        RECOGNIZE_BUTTON_ID,
        "写真を選んで文字を読む",
        request_recognize,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recognizing() -> ImageRecognizeState {
        let mut s = ImageRecognizeState::default();
        request_recognize(&mut s);
        mark_recognizing(&mut s);
        s
    }

    #[test]
    fn request_advances_generation_and_clears_previous_result() {
        let mut s = ImageRecognizeState {
            lines: vec!["old".into()],
            status: "1 行".into(),
            ..Default::default()
        };
        request_recognize(&mut s);
        assert_eq!(s.phase, ImageRecognizePhase::Requested);
        assert_eq!(s.generation, 1);
        assert!(s.lines.is_empty() && s.status.is_empty());
        assert_eq!(take_request(&s), Some(1));
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut s = recognizing();
        request_recognize(&mut s);
        assert_eq!(s.phase, ImageRecognizePhase::Recognizing);
        assert_eq!(s.generation, 1);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn success_shows_line_count() {
        let mut s = recognizing();
        finish_recognize(&mut s, 1, Ok(Some(vec!["a".into(), "b".into()])));
        assert_eq!(s.phase, ImageRecognizePhase::Idle);
        assert_eq!(s.status, "2 行");
        assert_eq!(s.lines, ["a", "b"]);
        assert!(s.error.is_empty());
    }

    #[test]
    fn empty_result_is_no_text_success_not_error() {
        let mut s = recognizing();
        finish_recognize(&mut s, 1, Ok(Some(vec![])));
        assert_eq!(s.status, NO_TEXT_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn cancel_is_not_failure_and_clears_lines() {
        let mut s = recognizing();
        finish_recognize(&mut s, 1, Ok(None));
        assert_eq!(s.status, CANCELLED_STATUS);
        assert!(s.error.is_empty() && s.lines.is_empty());
    }

    #[test]
    fn each_failure_sets_error_only() {
        let cases = [
            (
                RecognizeFailure::Unsupported,
                UNSUPPORTED_STATUS.to_string(),
            ),
            (
                RecognizeFailure::LanguageUnavailable,
                LANGUAGE_UNAVAILABLE_STATUS.to_string(),
            ),
            (
                RecognizeFailure::InvalidInput,
                INVALID_INPUT_STATUS.to_string(),
            ),
            (
                RecognizeFailure::Failed("x".into()),
                format!("{FAILED_PREFIX}x"),
            ),
        ];
        for (failure, expected) in cases {
            let mut s = recognizing();
            s.lines = vec!["stale".into()];
            finish_recognize(&mut s, 1, Err(failure));
            assert_eq!(s.error, expected);
            assert!(s.status.is_empty() && s.lines.is_empty());
        }
    }

    #[test]
    fn stale_generation_and_idle_completion_are_ignored() {
        let mut s = recognizing();
        finish_recognize(&mut s, 99, Ok(Some(vec!["x".into()])));
        assert_eq!(s.phase, ImageRecognizePhase::Recognizing);
        assert!(s.lines.is_empty());
        let mut idle = ImageRecognizeState::default();
        finish_recognize(&mut idle, 0, Ok(Some(vec!["x".into()])));
        assert!(idle.lines.is_empty());
    }

    #[test]
    fn mark_recognizing_only_from_requested() {
        let mut s = ImageRecognizeState::default();
        mark_recognizing(&mut s);
        assert_eq!(s.phase, ImageRecognizePhase::Idle);
    }

    #[test]
    fn button_only_when_idle() {
        let idle = ImageRecognizeState::default();
        let busy = recognizing();
        assert_eq!(current_screen(&idle).len(), current_screen(&busy).len() + 1);
    }
}
