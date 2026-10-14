//! Phase 11 の拡充ループ（親 #1135）型サンプル「画面の録画・キャプチャ」（Issue #1335）。
//! 「録画開始」→ 録画中（経過秒数）→「停止」または [`MAX_CAPTURE_MS`] 到達で自動停止 →
//! 結果（バイト数・mime・録画時間）または失敗理由・Unsupported を表示する 1 画面。
//! `FANDHE_PROXY_APP=screen_capture` で選ばれ、`fandhe-egui-proxy` の `screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod screen_capture;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。実際の録画・同意 UI は
//! `fandhe-egui-proxy` の effect ドライバ（`device/screen_capture.rs`）が
//! [`take_start_request`] / [`take_stop_request`] で要求を観測し、[`finish_capture`] で
//! 結果を注入する（`mic_record` と同型の pull 型）。
//!
//! 文言・定数は全 OS 分（Web 固有の案内を含む）をここで持つ。後続 OS の実装はこの画面を
//! 触らない。
//!
//! ```text
//! Idle --「録画開始」--> Requested, generation += 1
//!   --take_start_request → mark_recording--> Recording
//!   --tick（毎フレーム、上限到達で StopRequested）--> Recording
//!   --「停止」--> StopRequested
//!   --take_stop_request → mark_stopping--> Stopping
//!   --finish_capture（世代一致のみ反映）--> Idle + 結果表示
//! ```
//!
//! 停止操作は世代を進めない（同じ録画セッションの継続）。

use crate::{Screen, WidgetId};

const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 録画時間の上限（ミリ秒、10 秒固定）。ネイティブ側の自動停止と [`tick`] の期限をこの
/// 1 定数で揃える。
pub const MAX_CAPTURE_MS: u64 = 10_000;

/// 待機中の案内。
pub const IDLE_STATUS: &str = "待機";
/// 同意 UI を閉じた・拒否した際の案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 失敗時の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 環境が画面録画に対応しない場合の案内（失敗文言に混ぜない）。Web の非対応ブラウザでも使う。
pub const UNSUPPORTED_TEXT: &str = "この環境では画面の録画を使えません";
/// Web で利用者操作なしに始めた際（`InvalidStateError`）の案内。
pub const NEEDS_USER_GESTURE_TEXT: &str = "ボタン操作から始めてください";

/// 録画操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapturePhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// 「録画開始」が押され、effect ドライバへ未発送。
    Requested,
    /// effect ドライバが開始した。
    Recording,
    /// 「停止」または上限到達。effect ドライバへ未発送。
    StopRequested,
    /// 停止を依頼し、結果待ち。
    Stopping,
}

/// 録画結果の要約（映像の中身は保持しない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSummary {
    /// 映像のバイト数。
    pub size_bytes: u64,
    /// MIME タイプ。
    pub mime: String,
    /// 録画時間（ミリ秒）。
    pub duration_ms: u64,
}

/// [`finish_capture`] へ渡す正常系の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// 録画できた。
    Captured(CaptureSummary),
    /// 同意 UI を閉じた・拒否した。
    Cancelled,
    /// この環境では使えない。
    Unsupported,
}

/// 開始要求（[`take_start_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartRequest {
    /// [`finish_capture`] へ渡す世代番号。
    pub generation: u64,
    /// 録画時間の上限（常に [`MAX_CAPTURE_MS`]）。
    pub max_duration_ms: u64,
}

/// 停止要求（[`take_stop_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopRequest {
    /// 世代番号。
    pub generation: u64,
}

/// 画面の録画・キャプチャ画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScreenCaptureState {
    /// 進行状態。
    pub phase: CapturePhase,
    /// 直近に立てた開始要求の世代番号。
    pub generation: u64,
    /// 経過ミリ秒（[`tick`] のみが更新、[`MAX_CAPTURE_MS`] 以下）。
    pub elapsed_ms: u64,
    /// 直近の結果（通常色）。
    pub status: String,
    /// 失敗の案内（エラー色）。正常系は空。
    pub error: String,
}

/// 「録画開始」ハンドラ。`Idle` 以外は no-op。
pub fn start_capture(state: &mut ScreenCaptureState) {
    if state.phase != CapturePhase::Idle {
        return;
    }
    state.phase = CapturePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.elapsed_ms = 0;
    state.status = String::new();
    state.error = String::new();
}

/// 未発送の開始要求を観測する（状態は変えない）。
pub fn take_start_request(state: &ScreenCaptureState) -> Option<StartRequest> {
    (state.phase == CapturePhase::Requested).then_some(StartRequest {
        generation: state.generation,
        max_duration_ms: MAX_CAPTURE_MS,
    })
}

/// `Requested` → `Recording`。
pub fn mark_recording(state: &mut ScreenCaptureState) {
    if state.phase == CapturePhase::Requested {
        state.phase = CapturePhase::Recording;
    }
}

/// 経過時間を進める（`Recording` のみ）。上限到達で `StopRequested` へ自動遷移する。
pub fn tick(state: &mut ScreenCaptureState, dt_ms: u64) {
    if state.phase != CapturePhase::Recording {
        return;
    }
    state.elapsed_ms = (state.elapsed_ms + dt_ms).min(MAX_CAPTURE_MS);
    if state.elapsed_ms >= MAX_CAPTURE_MS {
        state.phase = CapturePhase::StopRequested;
    }
}

/// 「停止」ハンドラ。`Recording` 以外は no-op。
pub fn request_stop(state: &mut ScreenCaptureState) {
    if state.phase == CapturePhase::Recording {
        state.phase = CapturePhase::StopRequested;
    }
}

/// 未発送の停止要求を観測する（状態は変えない）。
pub fn take_stop_request(state: &ScreenCaptureState) -> Option<StopRequest> {
    (state.phase == CapturePhase::StopRequested).then_some(StopRequest {
        generation: state.generation,
    })
}

/// `StopRequested` → `Stopping`。
pub fn mark_stopping(state: &mut ScreenCaptureState) {
    if state.phase == CapturePhase::StopRequested {
        state.phase = CapturePhase::Stopping;
    }
}

/// 結果を反映する。`Idle` 中・世代不一致の完了は捨てる。
pub fn finish_capture(
    state: &mut ScreenCaptureState,
    generation: u64,
    result: Result<CaptureOutcome, String>,
) {
    if state.phase == CapturePhase::Idle || state.generation != generation {
        return;
    }
    state.phase = CapturePhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CaptureOutcome::Captured(s)) => {
            state.status = format!(
                "結果: {}（{} バイト）・{}・{} ミリ秒",
                format_size(s.size_bytes),
                s.size_bytes,
                s.mime,
                s.duration_ms
            );
        }
        Ok(CaptureOutcome::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Ok(CaptureOutcome::Unsupported) => state.status = UNSUPPORTED_TEXT.to_string(),
        Err(message) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// バイト数を 10 進・小数 1 桁で整形する（整形はこの 1 か所）。
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1_000.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// 案内文言。録画中は経過秒数、停止中は「停止中」、それ以外は直近の結果。
pub fn status_text(state: &ScreenCaptureState) -> String {
    match state.phase {
        CapturePhase::Idle if state.status.is_empty() => IDLE_STATUS.to_string(),
        CapturePhase::Idle => state.status.clone(),
        CapturePhase::Stopping => "停止中".to_string(),
        _ => format!("録画中 {}秒", state.elapsed_ms / 1000),
    }
}

/// 失敗の案内文言。
pub fn error_text(state: &ScreenCaptureState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` は「録画開始」、`Recording` は「停止」を出し、それ以外は
/// 二重押し防止のためボタンを外す。
pub fn current_screen(state: &ScreenCaptureState) -> Screen<ScreenCaptureState> {
    let screen = Screen::new("画面の録画")
        .dynamic_text(status_text)
        .error_text(error_text);
    match state.phase {
        CapturePhase::Idle => screen.button(START_BUTTON_ID, "録画開始", start_capture),
        CapturePhase::Recording => screen.button(STOP_BUTTON_ID, "停止", request_stop),
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> ScreenCaptureState {
        let mut s = ScreenCaptureState::default();
        start_capture(&mut s);
        mark_recording(&mut s);
        s
    }

    #[test]
    fn start_advances_generation_and_ignores_double_start() {
        let mut s = ScreenCaptureState::default();
        start_capture(&mut s);
        assert_eq!((s.phase, s.generation), (CapturePhase::Requested, 1));
        start_capture(&mut s);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn take_start_request_only_when_requested() {
        let mut s = ScreenCaptureState::default();
        assert_eq!(take_start_request(&s), None);
        start_capture(&mut s);
        assert_eq!(
            take_start_request(&s).unwrap().max_duration_ms,
            MAX_CAPTURE_MS
        );
        mark_recording(&mut s);
        assert_eq!(take_start_request(&s), None);
    }

    #[test]
    fn tick_auto_stops_at_limit() {
        let mut s = recording();
        tick(&mut s, 9_999);
        assert_eq!(s.phase, CapturePhase::Recording);
        tick(&mut s, 5_000);
        assert_eq!(s.elapsed_ms, MAX_CAPTURE_MS);
        assert_eq!(s.phase, CapturePhase::StopRequested);
    }

    #[test]
    fn stop_does_not_advance_generation() {
        let mut s = recording();
        request_stop(&mut s);
        assert_eq!(take_stop_request(&s).unwrap().generation, 1);
        mark_stopping(&mut s);
        assert_eq!((s.phase, s.generation), (CapturePhase::Stopping, 1));
    }

    #[test]
    fn stale_generation_and_idle_finish_are_ignored() {
        let mut s = recording();
        finish_capture(&mut s, 99, Ok(CaptureOutcome::Cancelled));
        assert_eq!(s.phase, CapturePhase::Recording);
        let mut idle = ScreenCaptureState::default();
        finish_capture(&mut idle, 0, Ok(CaptureOutcome::Cancelled));
        assert_eq!(idle.status, "");
    }

    #[test]
    fn outcomes_are_not_conflated() {
        let mut s = recording();
        finish_capture(&mut s, 1, Ok(CaptureOutcome::Cancelled));
        assert_eq!(
            (s.status.as_str(), s.error.as_str()),
            (CANCELLED_STATUS, "")
        );

        let mut s = recording();
        finish_capture(&mut s, 1, Ok(CaptureOutcome::Unsupported));
        assert_eq!(
            (s.status.as_str(), s.error.as_str()),
            (UNSUPPORTED_TEXT, "")
        );

        let mut s = recording();
        finish_capture(&mut s, 1, Err("x".into()));
        assert_eq!(s.error, format!("{FAILED_PREFIX}x"));
        assert_eq!(s.status, "");

        let mut s = recording();
        finish_capture(
            &mut s,
            1,
            Ok(CaptureOutcome::Captured(CaptureSummary {
                size_bytes: 1_500,
                mime: "video/mp4".into(),
                duration_ms: 2_000,
            })),
        );
        assert_eq!(s.phase, CapturePhase::Idle);
        assert!(s.status.contains("1.5 KB") && s.status.contains("1500 バイト"));
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1_500_000), "1.5 MB");
        assert_eq!(format_size(2_000_000_000), "2.0 GB");
    }

    #[test]
    fn status_text_per_phase() {
        let mut s = recording();
        s.elapsed_ms = 3_400;
        assert_eq!(status_text(&s), "録画中 3秒");
        s.phase = CapturePhase::Stopping;
        assert_eq!(status_text(&s), "停止中");
        assert_eq!(status_text(&ScreenCaptureState::default()), IDLE_STATUS);
    }
}
