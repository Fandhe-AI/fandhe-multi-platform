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
//! Windows は OS の撮影 UI が無いため、アプリが組む撮影パネル（準備完了・録画中のボタン）を
//! `Running` 中の画面に出す（Issue #1150）。パネルの状態（[`PanelPhase`]）は OS 層（ワーカー）が
//! 観測した事実を effect が [`set_panel`] で写したもので、ボタンは命令（[`PanelCommand`]）を
//! 立てるだけ。状態を進めるのは OS の観測であり、画面側では進めない。
//!
//! 期限は持たない（OS の応答を正とする。Windows の録画 30 秒は OS 層が測る）。キャンセルは失敗ではなく状態として出す（契約表 10）。
//! 動画の中身・パス・OS のエラー文は持たない（契約表 8）。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_capture（Running かつ世代一致のみ）--> Idle（result に Ok(結果) か Err(失敗)）
//! ```

use crate::{Screen, WidgetId};

const CAPTURE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const START_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(3);
const CANCEL_BUTTON_ID: WidgetId = WidgetId::from_raw(4);

/// 長さの上限（秒、案内文言用）。`fandhe_device::video_capture::MAX_DURATION_SECS` と一致させる
/// （core は device を import しないため、一致は `fandhe-egui-proxy` のテストで縛る）。
pub const MAX_DURATION_SECS: u32 = 30;

/// タイトル。
pub const TITLE: &str = "動画の撮影";
/// ボタンの文言。
pub const CAPTURE_LABEL: &str = "動画を撮る";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "撮影画面で最長 30 秒の動画を撮り、長さ・寸法・サイズを表示します（対応は iOS・Android・macOS・Windows・Web。デスクトップのブラウザではファイル選択になります）";
/// 処理中の案内。
pub const RUNNING_STATUS: &str = "撮影画面を開いています…";
/// 撮影パネルの準備完了時の案内（Windows）。
pub const PANEL_READY_STATUS: &str =
    "カメラの準備ができました。録画を始めてください（最長 30 秒・音声は録りません）";
/// 録画中の案内（Windows）。
pub const PANEL_RECORDING_STATUS: &str = "録画中です（30 秒で自動で止まります）";
/// 開始中・停止中の案内（Windows）。
pub const PANEL_BUSY_STATUS: &str = "処理しています…";
/// 録画を始めるボタンの文言。
pub const START_RECORDING_LABEL: &str = "録画を始める";
/// 録画を止めるボタンの文言。
pub const STOP_LABEL: &str = "止める";
/// 撮影を取り消すボタンの文言。
pub const CANCEL_LABEL: &str = "キャンセル";
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
    /// 撮影 UI で撮った（iOS・Android・macOS・Windows）。
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

/// 撮影パネルの状態（Windows）。OS 層が観測した事実を写したもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelPhase {
    /// カメラの準備ができ、録画の開始待ち（「録画を始める」「キャンセル」）。
    Ready,
    /// 録画中（「止める」「キャンセル」）。
    Recording,
    /// 開始中・停止中（ボタンなし）。
    Busy,
}

/// 撮影パネルのボタンが立てる命令。OS 層（ワーカー）が状態と照合して受け付けるか捨てる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelCommand {
    /// 録画を始める。
    Start,
    /// 録画を止める。
    Stop,
    /// 取り消す。
    Cancel,
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
    /// 撮影パネルの状態（Windows）。`Running` の間だけ `Some`。
    pub panel: Option<PanelPhase>,
    /// 押されたが effect がまだ OS 層へ渡していないパネルの命令。
    pub panel_command: Option<PanelCommand>,
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
    state.panel = None;
    state.panel_command = None;
    true
}

/// 撮影パネルの状態を写す。「`Running` かつ同じ世代」のときだけ適用し、反映したかを返す。
pub fn set_panel(
    state: &mut VideoCaptureShowState,
    generation: u64,
    panel: Option<PanelPhase>,
) -> bool {
    if state.phase != Phase::Running || state.generation != generation {
        return false;
    }
    state.panel = panel;
    true
}

/// 「録画を始める」。パネルが `Ready` のときだけ命令を立てる。
pub fn press_start(state: &mut VideoCaptureShowState) {
    if state.panel == Some(PanelPhase::Ready) {
        state.panel_command = Some(PanelCommand::Start);
    }
}

/// 「止める」。パネルが `Recording` のときだけ命令を立てる。
pub fn press_stop(state: &mut VideoCaptureShowState) {
    if state.panel == Some(PanelPhase::Recording) {
        state.panel_command = Some(PanelCommand::Stop);
    }
}

/// 「キャンセル」。パネルが `Ready` / `Recording` のときだけ命令を立てる。
pub fn press_cancel(state: &mut VideoCaptureShowState) {
    if matches!(state.panel, Some(PanelPhase::Ready | PanelPhase::Recording)) {
        state.panel_command = Some(PanelCommand::Cancel);
    }
}

/// effect ドライバが未転送の命令を、世代つきで観測する（状態は変えない）。
pub fn take_panel_command(state: &VideoCaptureShowState) -> Option<(u64, PanelCommand)> {
    (state.phase == Phase::Running)
        .then_some(state.panel_command)
        .flatten()
        .map(|c| (state.generation, c))
}

/// 転送済みの命令を消す。世代と命令が一致するときだけ（後から立った別の命令は消さない）。
pub fn clear_panel_command(
    state: &mut VideoCaptureShowState,
    generation: u64,
    command: PanelCommand,
) {
    if state.generation == generation && state.panel_command == Some(command) {
        state.panel_command = None;
    }
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
    if state.phase == Phase::Running {
        match state.panel {
            Some(PanelPhase::Ready) => return PANEL_READY_STATUS.to_string(),
            Some(PanelPhase::Recording) => return PANEL_RECORDING_STATUS.to_string(),
            Some(PanelPhase::Busy) => return PANEL_BUSY_STATUS.to_string(),
            None => {}
        }
    }
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

/// 画面を組み立てる。通常のボタンは処理中以外のときだけ出す。`Running` 中は撮影パネルの状態に
/// 応じたボタン（Windows）だけを出す。
pub fn current_screen(state: &VideoCaptureShowState) -> Screen<VideoCaptureShowState> {
    let screen = Screen::new(TITLE)
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase == Phase::Running {
        return match state.panel {
            Some(PanelPhase::Ready) => screen
                .button(START_BUTTON_ID, START_RECORDING_LABEL, press_start)
                .button(CANCEL_BUTTON_ID, CANCEL_LABEL, press_cancel),
            Some(PanelPhase::Recording) => screen
                .button(STOP_BUTTON_ID, STOP_LABEL, press_stop)
                .button(CANCEL_BUTTON_ID, CANCEL_LABEL, press_cancel),
            _ => screen,
        };
    }
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

    #[test]
    fn panel_buttons_follow_phase() {
        let count = |s: &VideoCaptureShowState| current_screen(s).len() - 3;
        let mut s = running();
        assert_eq!(count(&s), 0);
        assert!(set_panel(&mut s, 1, Some(PanelPhase::Ready)));
        assert_eq!(count(&s), 2);
        assert_eq!(status_text(&s), PANEL_READY_STATUS);
        set_panel(&mut s, 1, Some(PanelPhase::Recording));
        assert_eq!(count(&s), 2);
        assert_eq!(status_text(&s), PANEL_RECORDING_STATUS);
        set_panel(&mut s, 1, Some(PanelPhase::Busy));
        assert_eq!(count(&s), 0);
        assert_eq!(status_text(&s), PANEL_BUSY_STATUS);
    }

    #[test]
    fn commands_only_when_phase_matches() {
        let mut s = running();
        press_start(&mut s);
        press_stop(&mut s);
        press_cancel(&mut s);
        assert_eq!(s.panel_command, None);
        set_panel(&mut s, 1, Some(PanelPhase::Ready));
        press_stop(&mut s);
        assert_eq!(s.panel_command, None);
        press_start(&mut s);
        assert_eq!(take_panel_command(&s), Some((1, PanelCommand::Start)));
        // 別の命令は消さない
        clear_panel_command(&mut s, 1, PanelCommand::Cancel);
        assert_eq!(s.panel_command, Some(PanelCommand::Start));
        clear_panel_command(&mut s, 1, PanelCommand::Start);
        assert_eq!(s.panel_command, None);
        set_panel(&mut s, 1, Some(PanelPhase::Recording));
        press_start(&mut s);
        assert_eq!(s.panel_command, None);
        press_stop(&mut s);
        assert_eq!(s.panel_command, Some(PanelCommand::Stop));
        set_panel(&mut s, 1, Some(PanelPhase::Busy));
        press_cancel(&mut s);
        assert_eq!(s.panel_command, Some(PanelCommand::Stop));
    }

    #[test]
    fn stale_set_panel_is_ignored_and_finish_clears_panel() {
        let mut s = running();
        assert!(!set_panel(&mut s, 9, Some(PanelPhase::Ready)));
        assert_eq!(s.panel, None);
        set_panel(&mut s, 1, Some(PanelPhase::Ready));
        press_cancel(&mut s);
        assert!(finish_capture(&mut s, 1, Ok(CoreOutcome::Cancelled)));
        assert_eq!((s.panel, s.panel_command), (None, None));
        assert_eq!(take_panel_command(&s), None);
        // Running でなければ写さない
        assert!(!set_panel(&mut s, 1, Some(PanelPhase::Ready)));
    }
}
