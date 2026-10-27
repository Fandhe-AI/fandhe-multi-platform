//! Phase 11（端末機能の拡充ループ、親 #191）型サンプル「端末内での画像生成」（Issue #975）。
//! OS の画像生成画面（Image Playground）で作り、確定した 1 枚を表示する 1 画面。
//! `FANDHE_PROXY_APP=image_generation_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod image_generation_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/image_generation.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_generate`] で状態を進める。全対象 OS（macOS・iOS）の分の
//! 状態・文言はここで定義するため、iOS の子 issue（#976）はこのファイルを触らない。
//!
//! キャンセルは失敗ではなく状態として出す（契約表 10）。画素・プロンプト・OS のエラー文は
//! エラー文に含めない（契約表 8）。画素は [`RgbaPixels`] として状態に持ち、画像ノードで描く。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_generate（Running かつ世代一致のみ）--> Idle（result に Ok(結果) か Err(失敗)）
//! ```

use crate::{RgbaPixels, Screen, WidgetId};

const GENERATE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// タイトル。
pub const TITLE: &str = "端末内での画像生成";
/// ボタンの文言。
pub const GENERATE_LABEL: &str = "画像を生成";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "OS の画像生成画面で画像を作り、確定した 1 枚を表示します（対応は macOS・iOS の Apple Intelligence 対応機種のみ。Android・Windows・Web では使えません）";
/// 処理中の案内。
pub const RUNNING_STATUS: &str = "画像生成画面を開いています…";
/// キャンセルしたときの文言（エラーではなく状態として出す）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "生成できませんでした: ";
/// 非対応のときの案内（接頭辞なし）。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 表示先がないときの案内。
pub const NO_WINDOW_MESSAGE: &str = "画像生成画面を出す窓がありません";
/// 画像を復号できないときの案内。
pub const UNDECODABLE_MESSAGE: &str = "画像を表示できませんでした";
/// 画像が上限を超えたときの案内。
pub const TOO_LARGE_MESSAGE: &str = "画像が大きすぎます";
/// 前面でないときの案内。
pub const NOT_FOREGROUND_MESSAGE: &str = "アプリが前面にありませんでした";
/// 期限切れのときの案内。
pub const TIMED_OUT_MESSAGE: &str = "時間内に開けませんでした";
/// それ以外の失敗の案内。
pub const INTERNAL_MESSAGE: &str = "原因不明のエラーが起きました";
/// 画像の代替テキスト。
pub const IMAGE_ALT: &str = "生成した画像";

/// 「{w}×{h} の画像を表示しています」の文言。
fn generated_status(p: &RgbaPixels) -> String {
    format!("{}×{} の画像を表示しています", p.width(), p.height())
}

/// 生成できなかった理由（`fandhe_device::image_generation::ImageGenerationFailure` から写す責務は
/// effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFailure {
    /// 非対応。
    Unsupported,
    /// 表示先がない。
    NoWindow,
    /// 画像を復号できない。
    Undecodable,
    /// 画像が上限超過。
    TooLarge,
    /// 前面でない。
    NotForeground,
    /// 生成画面を出すまでの期限切れ。
    TimedOut,
    /// それ以外。
    Internal,
}

/// 成功側の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 生成した（検証済みの画素）。
    Generated(RgbaPixels),
    /// 利用者が閉じた。
    Cancelled,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作、または完了後。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 生成画面を開いている（利用者が確定するか閉じるまで）。
    Running,
}

/// 画像生成画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageGenerationShowState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の結果。未完了なら `None`。
    pub result: Option<Result<CoreOutcome, CoreFailure>>,
}

fn is_busy(state: &ImageGenerationShowState) -> bool {
    state.phase != Phase::Idle
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_generate(state: &mut ImageGenerationShowState) {
    if is_busy(state) {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.result = None;
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。世代は [`finish_generate`] へ
/// そのまま渡す。
pub fn take_request(state: &ImageGenerationShowState) -> Option<u64> {
    (state.phase == Phase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut ImageGenerationShowState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み・未要求）は無視する。反映したかどうかを返す。
pub fn finish_generate(
    state: &mut ImageGenerationShowState,
    generation: u64,
    result: Result<CoreOutcome, CoreFailure>,
) -> bool {
    if state.phase != Phase::Running || state.generation != generation {
        return false;
    }
    state.phase = Phase::Idle;
    state.result = Some(result);
    true
}

/// 状態の案内文言（処理中、生成した、キャンセル）。
pub fn status_text(state: &ImageGenerationShowState) -> String {
    if is_busy(state) {
        return RUNNING_STATUS.to_string();
    }
    match &state.result {
        Some(Ok(CoreOutcome::Generated(p))) => generated_status(p),
        Some(Ok(CoreOutcome::Cancelled)) => CANCELLED_STATUS.to_string(),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。画素・OS の文言は含めない。
pub fn error_text(state: &ImageGenerationShowState) -> String {
    let Some(Err(f)) = state.result else {
        return String::new();
    };
    let message = match f {
        CoreFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        CoreFailure::NoWindow => NO_WINDOW_MESSAGE,
        CoreFailure::Undecodable => UNDECODABLE_MESSAGE,
        CoreFailure::TooLarge => TOO_LARGE_MESSAGE,
        CoreFailure::NotForeground => NOT_FOREGROUND_MESSAGE,
        CoreFailure::TimedOut => TIMED_OUT_MESSAGE,
        CoreFailure::Internal => INTERNAL_MESSAGE,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 画面を組み立てる。生成済みのときだけ画像ノードを置き、ボタンは処理中以外のときだけ出す
/// （取り消しボタンと代替入力は置かない）。
pub fn current_screen(state: &ImageGenerationShowState) -> Screen<ImageGenerationShowState> {
    let mut screen = Screen::new(TITLE)
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if let Some(Ok(CoreOutcome::Generated(p))) = &state.result {
        screen = screen.rgba_image(p.clone(), IMAGE_ALT);
    }
    if is_busy(state) {
        return screen;
    }
    screen.button(GENERATE_BUTTON_ID, GENERATE_LABEL, request_generate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> ImageGenerationShowState {
        let mut s = ImageGenerationShowState::default();
        request_generate(&mut s);
        mark_running(&mut s);
        s
    }

    fn pixels() -> RgbaPixels {
        RgbaPixels::new(2, 1, vec![0u8; 8]).unwrap()
    }

    const ALL: [CoreFailure; 7] = [
        CoreFailure::Unsupported,
        CoreFailure::NoWindow,
        CoreFailure::Undecodable,
        CoreFailure::TooLarge,
        CoreFailure::NotForeground,
        CoreFailure::TimedOut,
        CoreFailure::Internal,
    ];

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = ImageGenerationShowState::default();
        request_generate(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request_generate(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        assert_eq!(take_request(&s), None);
        request_generate(&mut s);
        assert_eq!(s.generation, 1);
        assert!(finish_generate(&mut s, 1, Ok(CoreOutcome::Cancelled)));
        assert_eq!(s.phase, Phase::Idle);
        request_generate(&mut s);
        assert_eq!(s.generation, 2);
        assert_eq!(s.result, None);
    }

    #[test]
    fn stale_and_idle_finish_are_ignored() {
        let mut s = running();
        assert!(!finish_generate(&mut s, 0, Ok(CoreOutcome::Cancelled)));
        assert_eq!(s.phase, Phase::Running);
        assert!(finish_generate(&mut s, 1, Err(CoreFailure::TimedOut)));
        // 期限で確定済みの後に届いた成功は適用しない
        assert!(!finish_generate(
            &mut s,
            1,
            Ok(CoreOutcome::Generated(pixels()))
        ));
        assert_eq!(s.result, Some(Err(CoreFailure::TimedOut)));
        let mut idle = ImageGenerationShowState::default();
        assert!(!finish_generate(&mut idle, 0, Ok(CoreOutcome::Cancelled)));
        assert_eq!(idle.result, None);
    }

    #[test]
    fn cancelled_is_a_state_not_an_error() {
        let mut s = running();
        finish_generate(&mut s, 1, Ok(CoreOutcome::Cancelled));
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), CANCELLED_STATUS);
    }

    #[test]
    fn every_failure_shows_error_and_no_status() {
        for f in ALL {
            let mut s = running();
            finish_generate(&mut s, 1, Err(f));
            let t = error_text(&s);
            assert!(!t.is_empty(), "{f:?}");
            assert_eq!(status_text(&s), "", "{f:?}");
            if f == CoreFailure::Unsupported {
                assert_eq!(t, UNSUPPORTED_MESSAGE);
            } else {
                assert!(t.starts_with(FAILED_PREFIX), "{t}");
            }
        }
    }

    #[test]
    fn generated_shows_size() {
        let mut s = running();
        finish_generate(&mut s, 1, Ok(CoreOutcome::Generated(pixels())));
        assert_eq!(status_text(&s), "2×1 の画像を表示しています");
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn image_node_only_when_generated_and_button_hidden_while_busy() {
        // 固定 3 ノード（案内・状態・失敗）+ ボタンまたは画像
        let count = |s: &ImageGenerationShowState| current_screen(s).len() - 3;
        let mut s = ImageGenerationShowState::default();
        assert_eq!(count(&s), 1);
        request_generate(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_generate(&mut s, 1, Err(CoreFailure::Internal));
        assert_eq!(count(&s), 1);
        request_generate(&mut s);
        mark_running(&mut s);
        finish_generate(&mut s, 2, Ok(CoreOutcome::Generated(pixels())));
        // ボタン + 画像
        assert_eq!(count(&s), 2);
    }
}
