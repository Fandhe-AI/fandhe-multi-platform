//! Phase 11（端末機能の拡充ループ、親 #182）型サンプル「ファイル・動画のプレビューとサムネイル」
//! （Issue #917）。固定のサンプル画像と種類不明のファイルに対する 3 操作（サムネイル・種類不明ファイルの
//! サムネイル・プレビュー）を持つ 1 画面。`FANDHE_PROXY_APP=file_preview` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod file_preview_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/file_preview.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_working`]・[`finish`] で状態を進める。全対象 OS で同じ UI・文言を使うため、
//! OS 別の子 issue はこのファイルを触らない。
//!
//! ```text
//! Idle --ボタン--> Requested(op) (generation += 1, 前回の結果を消す)
//!   --effect--> Working(op)
//!   --finish（Idle 以外かつ世代一致のみ）--> Idle（outcome に結果）
//! ```
//!
//! 閉じた・サムネイルなし・表示アプリなし・対象外・非対応は通常色の案内で、失敗の色にしない。
//! `HandedOff` は「渡した」であって「表示した」ではない。

use crate::{RgbaPixels, Screen, WidgetId};

const THUMBNAIL_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const UNKNOWN_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const PREVIEW_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// サンプル画像のサムネイルボタンの文言。
pub const THUMBNAIL_LABEL: &str = "サンプル画像のサムネイル";
/// 種類不明のファイルのサムネイルボタンの文言。
pub const UNKNOWN_LABEL: &str = "種類不明のファイルのサムネイル";
/// プレビューボタンの文言。
pub const PREVIEW_LABEL: &str = "サンプル画像をプレビュー";
/// サムネイル作成中の案内。
pub const WORKING_THUMBNAIL_TEXT: &str = "作成中です";
/// プレビュー中の案内。
pub const WORKING_PREVIEW_TEXT: &str = "プレビュー中";
/// サムネイルが無いときの案内（通常色）。
pub const NO_THUMBNAIL_TEXT: &str = "このファイルにはサムネイルがありません";
/// プレビューを閉じたときの案内（通常色）。
pub const CLOSED_TEXT: &str = "プレビューを閉じました";
/// 他アプリへ渡したときの案内（通常色。「表示した」とは読めない文言にする）。
pub const HANDED_OFF_TEXT: &str = "表示するアプリに渡しました";
/// 表示できるアプリが無いときの案内（通常色）。
pub const NO_HANDLER_TEXT: &str = "表示できるアプリがありません";
/// プレビューの対象外 OS の案内（通常色）。
pub const TARGET_UNAVAILABLE_TEXT: &str =
    "この OS ではプレビューを表示できません（サムネイルのみ）";
/// 非対応環境の案内（通常色）。
pub const UNSUPPORTED_TEXT: &str = "この端末・OS 版では使えません";
/// 期限切れの案内（失敗の色）。
pub const TIMEOUT_TEXT: &str = "時間内にサムネイルを作れませんでした";
/// それ以外の失敗の案内（失敗の色）。
pub const FAILED_TEXT: &str = "できませんでした";
/// 画面の説明。
pub const NOTE: &str = "アプリが用意した固定のサンプル画像と種類不明のファイルだけを対象にします";
/// サムネイル画像の代替テキスト。
pub const IMAGE_ALT: &str = "ファイルのサムネイル";

/// 3 つの操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePreviewOp {
    /// サンプル画像のサムネイル。
    Thumbnail,
    /// 種類不明のファイルのサムネイル。
    ThumbnailUnknown,
    /// サンプル画像のプレビュー。
    Preview,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilePreviewPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested(FilePreviewOp),
    /// effect ドライバが処理中（[`mark_working`] 済み）。
    Working(FilePreviewOp),
}

/// 成功側の結果（失敗の色にしないものを含む）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePreviewOutcome {
    /// 作れたサムネイル。
    Thumbnail(RgbaPixels),
    /// サムネイルが無い。
    NoThumbnail,
    /// プレビューを閉じた。
    Closed,
    /// 他アプリへ渡した。
    HandedOff,
    /// 表示できるアプリが無い。
    NoHandler,
    /// この OS ではプレビューを表示しない。
    TargetUnavailable,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePreviewFailure {
    /// 非対応 OS・OS が古い。
    Unsupported,
    /// 期限切れ。
    Timeout,
    /// それ以外。
    Failed,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilePreviewState {
    /// 進行状態。
    pub phase: FilePreviewPhase,
    /// 直近の要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の結果。`None` は未実行または処理中。
    pub outcome: Option<Result<FilePreviewOutcome, FilePreviewFailure>>,
}

/// 要求を立てる。`Idle` 以外は二重要求防止のため no-op。前回の結果は消す。
pub fn request(state: &mut FilePreviewState, op: FilePreviewOp) {
    if state.phase != FilePreviewPhase::Idle {
        return;
    }
    state.phase = FilePreviewPhase::Requested(op);
    state.generation = state.generation.wrapping_add(1);
    state.outcome = None;
}

/// effect ドライバが未発送の要求の `(世代, 操作)` を観測する（状態は変えない）。
pub fn take_request(state: &FilePreviewState) -> Option<(u64, FilePreviewOp)> {
    match state.phase {
        FilePreviewPhase::Requested(op) => Some((state.generation, op)),
        _ => None,
    }
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_working(state: &mut FilePreviewState) {
    if let FilePreviewPhase::Requested(op) = state.phase {
        state.phase = FilePreviewPhase::Working(op);
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish(
    state: &mut FilePreviewState,
    generation: u64,
    result: Result<FilePreviewOutcome, FilePreviewFailure>,
) {
    if state.phase == FilePreviewPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = FilePreviewPhase::Idle;
    state.outcome = Some(result);
}

/// 通常色の案内（処理中・サムネイルなし・閉じた・渡した・対象外・非対応）。
pub fn status_text(state: &FilePreviewState) -> String {
    match state.phase {
        FilePreviewPhase::Requested(op) | FilePreviewPhase::Working(op) => {
            return match op {
                FilePreviewOp::Preview => WORKING_PREVIEW_TEXT,
                _ => WORKING_THUMBNAIL_TEXT,
            }
            .to_string();
        }
        FilePreviewPhase::Idle => {}
    }
    match &state.outcome {
        Some(Ok(FilePreviewOutcome::NoThumbnail)) => NO_THUMBNAIL_TEXT,
        Some(Ok(FilePreviewOutcome::Closed)) => CLOSED_TEXT,
        Some(Ok(FilePreviewOutcome::HandedOff)) => HANDED_OFF_TEXT,
        Some(Ok(FilePreviewOutcome::NoHandler)) => NO_HANDLER_TEXT,
        Some(Ok(FilePreviewOutcome::TargetUnavailable)) => TARGET_UNAVAILABLE_TEXT,
        Some(Err(FilePreviewFailure::Unsupported)) => UNSUPPORTED_TEXT,
        _ => "",
    }
    .to_string()
}

/// 失敗の案内（`TextRole::Error`）。期限切れとそれ以外の失敗だけ。
pub fn error_text(state: &FilePreviewState) -> String {
    if state.phase != FilePreviewPhase::Idle {
        return String::new();
    }
    match &state.outcome {
        Some(Err(FilePreviewFailure::Timeout)) => TIMEOUT_TEXT,
        Some(Err(FilePreviewFailure::Failed)) => FAILED_TEXT,
        _ => "",
    }
    .to_string()
}

fn request_thumbnail(s: &mut FilePreviewState) {
    request(s, FilePreviewOp::Thumbnail);
}

fn request_unknown(s: &mut FilePreviewState) {
    request(s, FilePreviewOp::ThumbnailUnknown);
}

fn request_preview(s: &mut FilePreviewState) {
    request(s, FilePreviewOp::Preview);
}

/// 画面を組み立てる。処理中はボタンを外して二重要求を防ぐ。
pub fn current_screen(state: &FilePreviewState) -> Screen<FilePreviewState> {
    let mut screen = Screen::new("ファイルのプレビュー")
        .text(NOTE)
        .dynamic_text(status_text)
        .error_text(error_text);
    if let Some(Ok(FilePreviewOutcome::Thumbnail(pixels))) = &state.outcome {
        screen = screen.rgba_image(pixels.clone(), IMAGE_ALT);
    }
    if state.phase != FilePreviewPhase::Idle {
        return screen;
    }
    screen
        .button(THUMBNAIL_BUTTON_ID, THUMBNAIL_LABEL, request_thumbnail)
        .button(UNKNOWN_BUTTON_ID, UNKNOWN_LABEL, request_unknown)
        .button(PREVIEW_BUTTON_ID, PREVIEW_LABEL, request_preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn working(op: FilePreviewOp) -> FilePreviewState {
        let mut s = FilePreviewState::default();
        request(&mut s, op);
        mark_working(&mut s);
        s
    }

    fn done(result: Result<FilePreviewOutcome, FilePreviewFailure>) -> FilePreviewState {
        let mut s = working(FilePreviewOp::Preview);
        let g = s.generation;
        finish(&mut s, g, result);
        s
    }

    #[test]
    fn request_while_busy_is_noop() {
        let mut s = working(FilePreviewOp::Thumbnail);
        let g = s.generation;
        request(&mut s, FilePreviewOp::Preview);
        assert_eq!(s.generation, g);
        assert_eq!(s.phase, FilePreviewPhase::Working(FilePreviewOp::Thumbnail));
    }

    #[test]
    fn request_clears_previous_outcome_and_bumps_generation() {
        let mut s = done(Ok(FilePreviewOutcome::Closed));
        let g = s.generation;
        request(&mut s, FilePreviewOp::Thumbnail);
        assert_eq!(s.generation, g + 1);
        assert_eq!(s.outcome, None);
        assert_eq!(take_request(&s), Some((g + 1, FilePreviewOp::Thumbnail)));
    }

    #[test]
    fn stale_and_idle_finish_are_ignored() {
        let mut s = working(FilePreviewOp::Preview);
        finish(&mut s, 99, Err(FilePreviewFailure::Failed));
        assert_eq!(s.phase, FilePreviewPhase::Working(FilePreviewOp::Preview));
        assert_eq!(s.outcome, None);

        let mut idle = FilePreviewState::default();
        finish(&mut idle, 0, Err(FilePreviewFailure::Failed));
        assert_eq!(idle.outcome, None);
    }

    #[test]
    fn non_failures_use_status_text_not_error_text() {
        for (r, text) in [
            (FilePreviewOutcome::NoThumbnail, NO_THUMBNAIL_TEXT),
            (FilePreviewOutcome::Closed, CLOSED_TEXT),
            (FilePreviewOutcome::HandedOff, HANDED_OFF_TEXT),
            (FilePreviewOutcome::NoHandler, NO_HANDLER_TEXT),
            (
                FilePreviewOutcome::TargetUnavailable,
                TARGET_UNAVAILABLE_TEXT,
            ),
        ] {
            let s = done(Ok(r));
            assert_eq!(error_text(&s), "");
            assert_eq!(status_text(&s), text);
        }
        let s = done(Err(FilePreviewFailure::Unsupported));
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), UNSUPPORTED_TEXT);
    }

    #[test]
    fn handed_off_does_not_claim_it_was_shown() {
        assert!(!HANDED_OFF_TEXT.contains("表示しました"));
    }

    #[test]
    fn timeout_and_failed_are_error_text() {
        let s = done(Err(FilePreviewFailure::Timeout));
        assert_eq!(error_text(&s), TIMEOUT_TEXT);
        assert_eq!(status_text(&s), "");
        let s = done(Err(FilePreviewFailure::Failed));
        assert_eq!(error_text(&s), FAILED_TEXT);
    }

    #[test]
    fn busy_screen_has_no_buttons_and_idle_has_three() {
        let busy = format!("{:?}", current_screen(&working(FilePreviewOp::Thumbnail)));
        let idle = format!("{:?}", current_screen(&FilePreviewState::default()));
        for label in [THUMBNAIL_LABEL, UNKNOWN_LABEL, PREVIEW_LABEL] {
            assert!(!busy.contains(label));
            assert!(idle.contains(label));
        }
    }

    #[test]
    fn working_text_depends_on_op() {
        assert_eq!(
            status_text(&working(FilePreviewOp::Preview)),
            WORKING_PREVIEW_TEXT
        );
        assert_eq!(
            status_text(&working(FilePreviewOp::ThumbnailUnknown)),
            WORKING_THUMBNAIL_TEXT
        );
    }
}
