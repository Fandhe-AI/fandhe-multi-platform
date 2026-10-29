//! Phase 11（端末機能の拡充ループ、親 #169）型サンプル「動画の撮影」（Issue #821）。
//! 撮影画面で動画を撮り、撮れた動画の長さ・寸法・ファイルサイズを表示する 1 画面。
//! `FANDHE_PROXY_APP=video_capture_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod video_capture_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しは
//! 本モジュール外（`fandhe-egui-proxy` の `device/video_capture.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_capture`] で状態を進める。全対象 OS の分の状態・文言は
//! ここで定義するため、OS 別の子 issue（#822・#823・#824）はこのファイルを触らない。
//! Web はファイル入力経由で「撮った動画」か「既存の動画」かを区別できないため、
//! [`ClipOrigin::Received`]（「受け取った動画」）で表示する。
//!
//! 期限は持たない（OS の応答を正とする）。キャンセルは失敗ではなく状態として出す（契約表 10）。
//! 動画の中身・パス・OS のエラー文は持たない（契約表 8）。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_capture（Running かつ世代一致のみ）--> Idle（result に Ok(結果) か Err(失敗)）
//! ```

use crate::{Screen, WidgetId};

const CAPTURE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 長さの上限（秒、案内文言用）。`fandhe_device::video_capture::MAX_DURATION_SECS` と一致させる
/// （core は device を import しないため、一致は `fandhe-egui-proxy` のテストで縛る）。
pub const MAX_DURATION_SECS: u32 = 30;

/// タイトル。
pub const TITLE: &str = "動画の撮影";
/// ボタンの文言。
pub const CAPTURE_LABEL: &str = "動画を撮る";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "撮影画面で最長 30 秒の動画を撮り、長さ・寸法・サイズを表示します（対応は iOS・Android・macOS・Web。Windows では使えません。デスクトップのブラウザではファイル選択になります）";
/// 処理中の案内。
pub const RUNNING_STATUS: &str = "撮影画面を開いています…";
/// 取り消したときの文言（エラーではなく状態として出す）。
pub const CANCELLED_STATUS: &str = "撮影を取り消しました";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "撮影できませんでした: ";
/// 非対応のときの案内（接頭辞なし）。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// カメラ権限が拒否されたときの案内。
pub const PERMISSION_DENIED_MESSAGE: &str =
    "カメラの使用が許可されていません（設定アプリで許可できます）";
/// 撮影アプリがないときの案内。
pub const NO_CAMERA_APP_MESSAGE: &str = "動画を撮れるアプリが見つかりません";
/// 前面でないときの案内。
pub const NOT_FOREGROUND_MESSAGE: &str = "アプリが前面にありませんでした";
/// 表示前に中断されたときの案内。
pub const INTERRUPTED_MESSAGE: &str = "表示の前に中断されました";
/// 動画が空のときの案内。
pub const EMPTY_MESSAGE: &str = "動画が保存されませんでした";
/// 動画の情報を読めないときの案内。
pub const UNREADABLE_MESSAGE: &str = "動画の情報を読み取れませんでした";
/// それ以外の失敗の案内。
pub const INTERNAL_MESSAGE: &str = "原因不明のエラーが起きました";
/// 撮影した動画の表示の接頭辞。
pub const RECORDED_PREFIX: &str = "撮った動画";
/// 受け取った動画（Web のファイル入力）の表示の接頭辞。
pub const RECEIVED_PREFIX: &str = "受け取った動画";
/// 長さを読めないときの表示。
pub const DURATION_UNKNOWN: &str = "長さを読めません";

/// 動画の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipOrigin {
    /// 撮影 UI で撮った（iOS・Android・macOS）。
    Recorded,
    /// ファイル入力で受け取った（Web。撮影か既存ファイルかを区別できない）。
    Received,
}

/// 表示中の動画の情報（動画の中身・パスは持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownClip {
    /// ファイルサイズ（バイト）。
    pub size_bytes: u64,
    /// 長さ（ミリ秒）。読めなければ `None`。
    pub duration_ms: Option<u64>,
    /// 表示の向きでの幅（画素）。
    pub width: u32,
    /// 表示の向きでの高さ（画素）。
    pub height: u32,
    /// 出どころ。
    pub origin: ClipOrigin,
}

/// 撮れなかった理由（`fandhe_device::video_capture::VideoCaptureFailure` から写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFailure {
    /// 非対応。
    Unsupported,
    /// カメラ権限の拒否・制限。
    PermissionDenied,
    /// 撮影アプリがない。
    NoCameraApp,
    /// 前面でない。
    NotForeground,
    /// 表示前の中断。
    Interrupted,
    /// 動画が空。
    Empty,
    /// 情報を読めない。
    Unreadable,
    /// それ以外。
    Internal,
}

/// 成功側の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 撮れた。
    Captured(ShownClip),
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
    /// 撮影 UI を開いている（利用者が撮るか閉じるまで）。
    Running,
}

/// 動画の撮影画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VideoCaptureShowState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の結果。未完了なら `None`。
    pub result: Option<Result<CoreOutcome, CoreFailure>>,
}

fn is_busy(state: &VideoCaptureShowState) -> bool {
    state.phase != Phase::Idle
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_capture(state: &mut VideoCaptureShowState) {
    if is_busy(state) {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.result = None;
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。世代は [`finish_capture`] へ
/// そのまま渡す。
pub fn take_request(state: &VideoCaptureShowState) -> Option<u64> {
    (state.phase == Phase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut VideoCaptureShowState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 確定済み・未要求）は無視する。反映したかどうかを返す。
pub fn finish_capture(
    state: &mut VideoCaptureShowState,
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

/// 長さの表示（ミリ秒 → 「12.3 秒」）。読めなければ [`DURATION_UNKNOWN`]。
pub fn format_duration(duration_ms: Option<u64>) -> String {
    match duration_ms {
        Some(ms) => format!("{}.{} 秒", ms / 1000, (ms % 1000) / 100),
        None => DURATION_UNKNOWN.to_string(),
    }
}

/// サイズの表示（バイト → 「4.2 MB」等）。1024 進。
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    let tenths = |unit: u64| {
        let t = bytes.saturating_mul(10) / unit;
        format!("{}.{}", t / 10, t % 10)
    };
    if bytes >= GB {
        format!("{} GB", tenths(GB))
    } else if bytes >= MB {
        format!("{} MB", tenths(MB))
    } else if bytes >= KB {
        format!("{} KB", tenths(KB))
    } else {
        format!("{bytes} B")
    }
}

/// 寸法の表示（「1080×1920」）。
pub fn format_dimensions(width: u32, height: u32) -> String {
    format!("{width}×{height}")
}

fn clip_status(clip: &ShownClip) -> String {
    let prefix = match clip.origin {
        ClipOrigin::Recorded => RECORDED_PREFIX,
        ClipOrigin::Received => RECEIVED_PREFIX,
    };
    format!(
        "{prefix}: {}、{}、{}",
        format_duration(clip.duration_ms),
        format_dimensions(clip.width, clip.height),
        format_size(clip.size_bytes)
    )
}

/// 状態の案内文言（処理中、撮れた、取り消し）。
pub fn status_text(state: &VideoCaptureShowState) -> String {
    if is_busy(state) {
        return RUNNING_STATUS.to_string();
    }
    match &state.result {
        Some(Ok(CoreOutcome::Captured(c))) => clip_status(c),
        Some(Ok(CoreOutcome::Cancelled)) => CANCELLED_STATUS.to_string(),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。動画・OS の文言は含めない。
pub fn error_text(state: &VideoCaptureShowState) -> String {
    let Some(Err(f)) = state.result else {
        return String::new();
    };
    let message = match f {
        CoreFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        CoreFailure::PermissionDenied => PERMISSION_DENIED_MESSAGE,
        CoreFailure::NoCameraApp => NO_CAMERA_APP_MESSAGE,
        CoreFailure::NotForeground => NOT_FOREGROUND_MESSAGE,
        CoreFailure::Interrupted => INTERRUPTED_MESSAGE,
        CoreFailure::Empty => EMPTY_MESSAGE,
        CoreFailure::Unreadable => UNREADABLE_MESSAGE,
        CoreFailure::Internal => INTERNAL_MESSAGE,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す（取り消しボタンは置かない）。
pub fn current_screen(state: &VideoCaptureShowState) -> Screen<VideoCaptureShowState> {
    let screen = Screen::new(TITLE)
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if is_busy(state) {
        return screen;
    }
    screen.button(CAPTURE_BUTTON_ID, CAPTURE_LABEL, request_capture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> VideoCaptureShowState {
        let mut s = VideoCaptureShowState::default();
        request_capture(&mut s);
        mark_running(&mut s);
        s
    }

    fn clip(origin: ClipOrigin, duration_ms: Option<u64>) -> ShownClip {
        ShownClip {
            size_bytes: 4_500_000,
            duration_ms,
            width: 1080,
            height: 1920,
            origin,
        }
    }

    const ALL: [CoreFailure; 8] = [
        CoreFailure::Unsupported,
        CoreFailure::PermissionDenied,
        CoreFailure::NoCameraApp,
        CoreFailure::NotForeground,
        CoreFailure::Interrupted,
        CoreFailure::Empty,
        CoreFailure::Unreadable,
        CoreFailure::Internal,
    ];

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = VideoCaptureShowState::default();
        request_capture(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request_capture(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        assert_eq!(take_request(&s), None);
        request_capture(&mut s);
        assert_eq!(s.generation, 1);
        assert!(finish_capture(&mut s, 1, Ok(CoreOutcome::Cancelled)));
        assert_eq!(s.phase, Phase::Idle);
        request_capture(&mut s);
        assert_eq!(s.generation, 2);
        assert_eq!(s.result, None);
    }

    #[test]
    fn stale_idle_and_requested_finish_are_ignored() {
        let mut s = running();
        assert!(!finish_capture(&mut s, 0, Ok(CoreOutcome::Cancelled)));
        assert_eq!(s.phase, Phase::Running);
        assert!(finish_capture(&mut s, 1, Err(CoreFailure::Interrupted)));
        // 確定済みの後に届いた成功は適用しない
        let c = Ok(CoreOutcome::Captured(clip(ClipOrigin::Recorded, None)));
        assert!(!finish_capture(&mut s, 1, c.clone()));
        assert_eq!(s.result, Some(Err(CoreFailure::Interrupted)));
        let mut idle = VideoCaptureShowState::default();
        assert!(!finish_capture(&mut idle, 0, c.clone()));
        assert_eq!(idle.result, None);
        let mut requested = VideoCaptureShowState::default();
        request_capture(&mut requested);
        assert!(!finish_capture(&mut requested, 1, c));
        assert_eq!(requested.phase, Phase::Requested);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = VideoCaptureShowState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn cancelled_is_a_state_not_an_error() {
        let mut s = running();
        finish_capture(&mut s, 1, Ok(CoreOutcome::Cancelled));
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), CANCELLED_STATUS);
    }

    #[test]
    fn every_failure_shows_error_and_no_status() {
        for f in ALL {
            let mut s = running();
            finish_capture(&mut s, 1, Err(f));
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
    fn origin_changes_prefix() {
        let mut a = running();
        finish_capture(
            &mut a,
            1,
            Ok(CoreOutcome::Captured(clip(
                ClipOrigin::Recorded,
                Some(12_345),
            ))),
        );
        assert_eq!(status_text(&a), "撮った動画: 12.3 秒、1080×1920、4.2 MB");
        assert_eq!(error_text(&a), "");
        let mut b = running();
        finish_capture(
            &mut b,
            1,
            Ok(CoreOutcome::Captured(clip(
                ClipOrigin::Received,
                Some(12_345),
            ))),
        );
        assert!(status_text(&b).starts_with(RECEIVED_PREFIX));
    }

    #[test]
    fn unknown_duration_is_stated() {
        let mut s = running();
        finish_capture(
            &mut s,
            1,
            Ok(CoreOutcome::Captured(clip(ClipOrigin::Recorded, None))),
        );
        assert!(status_text(&s).contains(DURATION_UNKNOWN));
    }

    #[test]
    fn formatters() {
        assert_eq!(format_duration(Some(0)), "0.0 秒");
        assert_eq!(format_duration(Some(30_000)), "30.0 秒");
        assert_eq!(format_duration(None), DURATION_UNKNOWN);
        assert_eq!(format_size(1), "1 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert!(format_size(u64::MAX).ends_with(" GB"));
        assert_eq!(format_dimensions(1920, 1080), "1920×1080");
    }

    #[test]
    fn button_hidden_while_busy() {
        // 固定 3 ノード（案内・状態・失敗）+ ボタン 1 つ
        let count = |s: &VideoCaptureShowState| current_screen(s).len() - 3;
        let mut s = VideoCaptureShowState::default();
        assert_eq!(count(&s), 1);
        request_capture(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_capture(&mut s, 1, Err(CoreFailure::Interrupted));
        assert_eq!(count(&s), 1);
    }
}
