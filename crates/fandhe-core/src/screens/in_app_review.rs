//! Phase 11 の拡充ループ型サンプル「アプリ内レビュー依頼」（Issue #563）。ボタン 1 つだけの 1 画面。
//! `FANDHE_PROXY_APP=in_app_review` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod in_app_review;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは effect ドライバ
//! （`device/in_app_review.rs`）が [`take_request`] で要求を観測して行い、[`finish_request`] で結果を
//! 注入する。全 OS 分の文言は (a) で確定し、後続の OS 別 PR は本ファイルに触れない。
//!
//! # 結果を混同しない理由
//!
//! 表示されたかは OS から返らない。成功文言は「依頼を出した」までに留め、非対応・失敗は error 側に出す。
//! 世代番号（`generation`）で、古い完了や `Idle` で届いた完了を無視する。

use crate::{Screen, WidgetId};

/// 「レビューを依頼する」ボタンの `WidgetId`。
const REQUEST_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 依頼を OS へ渡せたときの案内（表示の有無は保証しない）。
pub const REQUESTED_STATUS: &str = "レビュー依頼を出しました（表示するかは OS が決めます。年 3 回までなどの上限があり、TestFlight では出ません）";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境ではアプリ内レビュー依頼を使えません";
/// 失敗の固定文言。
pub const FAILED_STATUS: &str = "レビュー依頼を出せませんでした";
/// 固定の注記。
pub const NOTE_TEXT: &str = "iOS・macOS は OS のレビュー依頼を 1 回呼びます。表示されたかは返りません。\
Android・Windows は保留中、Web は対象外です。確認用にボタンで呼んでいます（本番では操作に直結させず流れの区切りで呼びます）";

/// 依頼の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewPhase {
    /// 未操作・前回の依頼が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_requesting`] 済み）。
    Requesting,
}

/// OS が判定した結果（写像は `fandhe-egui-proxy` 側の責務）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 依頼を OS へ渡した。
    Requested,
    /// この環境では非対応。
    Unsupported,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRequest {
    /// [`finish_request`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InAppReviewState {
    /// 進行状態。
    pub phase: ReviewPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 成功の案内（通常色）。
    pub status: String,
    /// 非対応・失敗（`TextRole::Error`）。
    pub error: String,
}

/// ボタンのハンドラ。`Idle` 以外は no-op（二重押し防止）。
pub fn request(state: &mut InAppReviewState) {
    if state.phase != ReviewPhase::Idle {
        return;
    }
    state.phase = ReviewPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &InAppReviewState) -> Option<ReviewRequest> {
    (state.phase == ReviewPhase::Requested).then_some(ReviewRequest {
        generation: state.generation,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_requesting(state: &mut InAppReviewState) {
    if state.phase == ReviewPhase::Requested {
        state.phase = ReviewPhase::Requesting;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_request(
    state: &mut InAppReviewState,
    generation: u64,
    result: Result<CoreOutcome, String>,
) {
    if state.phase == ReviewPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReviewPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::Requested) => state.status = REQUESTED_STATUS.to_string(),
        Ok(CoreOutcome::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(m) => state.error = m,
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &InAppReviewState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &InAppReviewState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外して二重押しを防ぐ。
pub fn current_screen(state: &InAppReviewState) -> Screen<InAppReviewState> {
    let screen = Screen::new("アプリ内レビュー依頼")
        .text(NOTE_TEXT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != ReviewPhase::Idle {
        return screen;
    }
    screen.button(REQUEST_BUTTON_ID, "レビューを依頼する", request)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requesting() -> InAppReviewState {
        let mut s = InAppReviewState::default();
        request(&mut s);
        mark_requesting(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, String>) -> (String, String) {
        let mut s = requesting();
        let g = s.generation;
        finish_request(&mut s, g, r);
        assert_eq!(s.phase, ReviewPhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn request_advances_generation_and_phase() {
        let mut s = InAppReviewState::default();
        request(&mut s);
        assert_eq!((s.phase, s.generation), (ReviewPhase::Requested, 1));
        assert_eq!(take_request(&s).unwrap().generation, 1);
        mark_requesting(&mut s);
        assert!(take_request(&s).is_none());
    }

    #[test]
    fn repress_in_flight_is_noop() {
        let mut s = requesting();
        let g = s.generation;
        request(&mut s);
        assert_eq!((s.phase, s.generation), (ReviewPhase::Requesting, g));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = requesting();
        let g = s.generation + 1;
        finish_request(&mut s, g, Ok(CoreOutcome::Requested));
        assert_eq!(s.phase, ReviewPhase::Requesting);
        assert!(s.status.is_empty());

        let mut idle = InAppReviewState::default();
        finish_request(&mut idle, 0, Ok(CoreOutcome::Requested));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn outcomes_are_distinct_and_only_requested_is_success() {
        let ok = shown(Ok(CoreOutcome::Requested));
        let un = shown(Ok(CoreOutcome::Unsupported));
        let ng = shown(Err(FAILED_STATUS.into()));
        assert_ne!(ok, un);
        assert_ne!(un, ng);
        assert!(!ok.0.is_empty() && ok.1.is_empty());
        for e in [&un, &ng] {
            assert!(e.0.is_empty() && !e.1.is_empty());
            assert!(!e.1.contains("出しました"));
        }
    }
}
