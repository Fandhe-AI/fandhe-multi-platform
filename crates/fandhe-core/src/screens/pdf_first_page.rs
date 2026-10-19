//! Phase 11（端末機能の拡充ループ、親 #107）型サンプル「PDF 表示」（Issue #469）。
//! 「サンプル PDF を表示」でアプリに埋め込んだ 3 ページの PDF の 1 ページ目を画像にして表示し、
//! 総ページ数を添える 1 画面。`FANDHE_PROXY_APP=pdf_first_page` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod pdf_first_page;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/pdf_viewer.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_loading`]・[`finish`] で状態を進める。全対象 OS で同じ UI・文言を使うため、
//! OS 別の子 issue はこのファイルを触らない。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Loading
//!   --finish（Idle 以外かつ世代一致のみ）--> Idle（outcome に成功か失敗）
//! ```
//!
//! 画像とページ数は成功のときだけ持つ。失敗を成功表示に丸めない。

use crate::{RgbaPixels, Screen, WidgetId};

const SHOW_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// ボタンの文言。
pub const BUTTON_LABEL: &str = "サンプル PDF を表示";
/// 処理中の案内。
pub const LOADING_TEXT: &str = "読み込み中です";
/// 非対応環境の案内（通常の文言として出す）。
pub const UNSUPPORTED_TEXT: &str = "この端末では PDF を表示できません";
/// PDF が壊れているときの案内。
pub const BROKEN_TEXT: &str = "PDF が壊れています";
/// 暗号化されていて開けないときの案内。
pub const ENCRYPTED_TEXT: &str = "パスワード付きの PDF は開けません";
/// ページが無いときの案内。
pub const EMPTY_TEXT: &str = "ページがありません";
/// 期限切れのときの案内。
pub const TIMEOUT_TEXT: &str = "時間内に表示できませんでした";
/// それ以外の失敗の案内。
pub const FAILED_TEXT: &str = "表示できませんでした";
/// 画面の説明。
pub const NOTE: &str = "アプリに埋め込んだ 3 ページの PDF の 1 ページ目だけを表示します";
/// 画像の代替テキスト。
pub const IMAGE_ALT: &str = "PDF の 1 ページ目";

/// 総ページ数の文言。
pub fn page_count_text(n: u32) -> String {
    format!("全 {n} ページ（1 ページ目を表示）")
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PdfPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// effect ドライバが処理中（[`mark_loading`] 済み）。
    Loading,
}

/// 表示する 1 ページ目と総ページ数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfShown {
    /// 1 ページ目の画像。
    pub pixels: RgbaPixels,
    /// 総ページ数。
    pub page_count: u32,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfFailure {
    /// 非対応 OS・OS が古い。
    Unsupported,
    /// PDF として読めない。
    Broken,
    /// 暗号化されていて開けない。
    Encrypted,
    /// ページが 0。
    Empty,
    /// 期限切れ。
    Timeout,
    /// それ以外。
    Failed,
}

/// PDF 表示画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PdfFirstPageState {
    /// 進行状態。
    pub phase: PdfPhase,
    /// 直近の要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の結果。`None` は未実行または処理中。
    pub outcome: Option<Result<PdfShown, PdfFailure>>,
}

/// 要求を立てる。`Idle` 以外は二重要求防止のため no-op。前回の結果は消す。
pub fn request(state: &mut PdfFirstPageState) {
    if state.phase != PdfPhase::Idle {
        return;
    }
    state.phase = PdfPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.outcome = None;
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。
pub fn take_request(state: &PdfFirstPageState) -> Option<u64> {
    (state.phase == PdfPhase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_loading(state: &mut PdfFirstPageState) {
    if state.phase == PdfPhase::Requested {
        state.phase = PdfPhase::Loading;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish(
    state: &mut PdfFirstPageState,
    generation: u64,
    result: Result<PdfShown, PdfFailure>,
) {
    if state.phase == PdfPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PdfPhase::Idle;
    state.outcome = Some(result);
}

/// 通常色の案内（処理中・総ページ数・非対応）。
pub fn status_text(state: &PdfFirstPageState) -> String {
    if state.phase != PdfPhase::Idle {
        return LOADING_TEXT.to_string();
    }
    match &state.outcome {
        Some(Ok(shown)) => page_count_text(shown.page_count),
        Some(Err(PdfFailure::Unsupported)) => UNSUPPORTED_TEXT.to_string(),
        _ => String::new(),
    }
}

/// 失敗の案内（`TextRole::Error`）。非対応は通常の案内なので空。
pub fn error_text(state: &PdfFirstPageState) -> String {
    if state.phase != PdfPhase::Idle {
        return String::new();
    }
    match &state.outcome {
        Some(Err(f)) => match f {
            PdfFailure::Unsupported => "",
            PdfFailure::Broken => BROKEN_TEXT,
            PdfFailure::Encrypted => ENCRYPTED_TEXT,
            PdfFailure::Empty => EMPTY_TEXT,
            PdfFailure::Timeout => TIMEOUT_TEXT,
            PdfFailure::Failed => FAILED_TEXT,
        }
        .to_string(),
        _ => String::new(),
    }
}

/// 画面を組み立てる。処理中はボタンを外して二重要求を防ぐ。
pub fn current_screen(state: &PdfFirstPageState) -> Screen<PdfFirstPageState> {
    let mut screen = Screen::new("PDF 表示")
        .text(NOTE)
        .dynamic_text(status_text)
        .error_text(error_text);
    if let Some(Ok(shown)) = &state.outcome {
        screen = screen.rgba_image(shown.pixels.clone(), IMAGE_ALT);
    }
    if state.phase != PdfPhase::Idle {
        return screen;
    }
    screen.button(SHOW_BUTTON_ID, BUTTON_LABEL, request)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown() -> PdfShown {
        PdfShown {
            pixels: RgbaPixels::new(1, 1, vec![0u8; 4]).unwrap(),
            page_count: 3,
        }
    }

    fn loading() -> PdfFirstPageState {
        let mut s = PdfFirstPageState::default();
        request(&mut s);
        mark_loading(&mut s);
        s
    }

    #[test]
    fn double_request_is_noop() {
        let mut s = PdfFirstPageState::default();
        request(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request(&mut s);
        assert_eq!(s.generation, 1);
        mark_loading(&mut s);
        request(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = loading();
        finish(&mut s, 99, Ok(shown()));
        assert_eq!(s.phase, PdfPhase::Loading);
        finish(&mut s, 1, Err(PdfFailure::Timeout));
        finish(&mut s, 1, Ok(shown()));
        assert_eq!(s.outcome, Some(Err(PdfFailure::Timeout)));
        let mut idle = PdfFirstPageState::default();
        finish(&mut idle, 0, Ok(shown()));
        assert_eq!(idle.outcome, None);
    }

    #[test]
    fn success_shows_image_and_page_count() {
        let mut s = loading();
        finish(&mut s, 1, Ok(shown()));
        assert_eq!(status_text(&s), "全 3 ページ（1 ページ目を表示）");
        assert_eq!(error_text(&s), "");
        assert!(format!("{:?}", current_screen(&s)).contains("RgbaImage"));
        request(&mut s);
        assert_eq!(s.outcome, None, "前回の結果は消える");
    }

    #[test]
    fn each_failure_has_text_and_no_image() {
        let cases = [
            (PdfFailure::Broken, BROKEN_TEXT),
            (PdfFailure::Encrypted, ENCRYPTED_TEXT),
            (PdfFailure::Empty, EMPTY_TEXT),
            (PdfFailure::Timeout, TIMEOUT_TEXT),
            (PdfFailure::Failed, FAILED_TEXT),
        ];
        for (f, text) in cases {
            let mut s = loading();
            finish(&mut s, 1, Err(f));
            assert_eq!(error_text(&s), text);
            assert!(!status_text(&s).contains("ページ"), "{f:?}");
            assert!(!format!("{:?}", current_screen(&s)).contains("RgbaImage"));
        }
        let mut s = loading();
        finish(&mut s, 1, Err(PdfFailure::Unsupported));
        assert_eq!(status_text(&s), UNSUPPORTED_TEXT);
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn busy_states_show_loading_without_button() {
        let mut s = PdfFirstPageState::default();
        request(&mut s);
        assert_eq!(status_text(&s), LOADING_TEXT);
        assert!(!format!("{:?}", current_screen(&s)).contains(BUTTON_LABEL));
        assert!(
            format!("{:?}", current_screen(&PdfFirstPageState::default())).contains(BUTTON_LABEL)
        );
    }
}
