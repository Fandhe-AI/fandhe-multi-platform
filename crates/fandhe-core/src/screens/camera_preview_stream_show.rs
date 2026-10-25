//! Phase 11 の拡充ループ（親 #168、Issue #812）型サンプル「カメラのライブ映像」。
//! 「映像を始める」で背面カメラのライブ映像を受け取り、最新 1 枚を縮めて表示し、受け取ったフレーム数・
//! 元の寸法・OS から読んだズーム値を出す。ズームは「1 倍」「2 倍」で切り替える。
//! `FANDHE_PROXY_APP=camera_preview_stream_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod camera_preview_stream_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/camera_preview_stream.rs` が [`take_start`] / [`take_zoom`] で要求を観測し、`apply_*` /
//! [`finish_zoom`] で結果を注入する。**起動時には映像を始めない**（ボタン起点）。全対象 OS（iOS・Android・
//! Web）の状態・文言をここで確定済みで、後続 OS の実装（#813〜#815）は本画面を触らない。
//!
//! # 世代
//!
//! 開始のたびに `generation` を進め、停止後・差し替え後に届いた通知は世代と phase で捨てる。
//! 停止は失敗ではない（失敗文言を出さない）。失敗・中断を映像中に畳まない。
//!
//! ```text
//! Idle/Stopped/Failed --「映像を始める」--> Starting(gen += 1)
//! Starting --apply_running--> Live <--apply_running / apply_interrupted--> Interrupted
//! Starting/Live/Interrupted --「止める」--> Stopped(User)
//! Starting --apply_start_failed--> Failed / Live・Interrupted --apply_runtime_failed--> Failed
//! Starting/Live/Interrupted --apply_stopped--> Stopped(Backgrounded / LeftScreen)
//! ```
//!
//! # ズーム
//!
//! 表示するズーム値は [`apply_frame`] が渡す OS から読んだ値だけで変わる（押したボタンの値は出さない）。
//! [`finish_zoom`] は要求を下ろすだけで表示値は変えない。

use crate::ui::{Node, RgbaPixels, Screen, WidgetId};

const TOGGLE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const ZOOM_ONE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const ZOOM_TWO_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 画面タイトル。
pub const TITLE: &str = "カメラのライブ映像";
/// 開始ボタン。
pub const START_LABEL: &str = "映像を始める";
/// 停止ボタン。
pub const STOP_LABEL: &str = "止める";
/// ズーム 1 倍ボタン。
pub const ZOOM_ONE_LABEL: &str = "1 倍";
/// ズーム 2 倍ボタン。
pub const ZOOM_TWO_LABEL: &str = "2 倍";
/// 初期の案内。
pub const IDLE_STATUS: &str = "「映像を始める」を押すとカメラの映像を表示します";
/// 開始待ち。
pub const STARTING_STATUS: &str = "開始を待っています";
/// 映像中。
pub const LIVE_STATUS: &str = "映像中";
/// 中断中。
pub const INTERRUPTED_STATUS: &str = "中断中（OS が一時停止しました）";
/// 利用者が止めた。
pub const STOPPED_STATUS: &str = "停止しました";
/// 背景へ移って止まった。
pub const BACKGROUNDED_STATUS: &str = "背景へ移ったため停止しました";
/// 画面を離れて止まった（Web の pagehide 用）。
pub const LEFT_SCREEN_STATUS: &str = "画面を離れたため停止しました";
/// 開始失敗の接頭辞（`{接頭辞}{理由}`）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 権限が無い。
pub const PERMISSION_DENIED_REASON: &str = "権限がありません";
/// カメラが無い。
pub const NO_CAMERA_REASON: &str = "カメラが見つかりません";
/// 他で使用中。
pub const IN_USE_REASON: &str = "他で使用中です";
/// 上記以外の開始失敗。
pub const OTHER_REASON: &str = "開始できません";
/// 実行中の失敗。
pub const RUNTIME_FAILED_STATUS: &str = "映像が止まりました";
/// 非対応。
pub const UNSUPPORTED_STATUS: &str = "この環境ではカメラの映像を使えません";
/// ズーム非対応の表示。
pub const ZOOM_UNAVAILABLE_TEXT: &str = "この環境では使えない";
/// ズーム失敗。
pub const ZOOM_FAILED_STATUS: &str = "ズームできませんでした";
/// ズームがこの環境で使えない。
pub const ZOOM_UNSUPPORTED_STATUS: &str = "この環境ではズームを使えません";
/// フレーム数の接頭辞。
pub const FRAMES_PREFIX: &str = "受け取ったフレーム: ";
/// 元寸法の接頭辞。
pub const SOURCE_PREFIX: &str = "元の寸法: ";
/// ズーム値の接頭辞。
pub const ZOOM_PREFIX: &str = "ズーム: ";
/// フレーム未着。
pub const NO_FRAME_TEXT: &str = "まだフレームが届いていません";
/// 映像の代替テキスト。
pub const FRAME_ALT: &str = "カメラのライブ映像";

/// 停止の見せ方（device の `StopReason` と利用者の停止を effect が写す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopView {
    /// 利用者の操作。
    User,
    /// 背景へ移った。
    Backgrounded,
    /// 画面を離れた。
    LeftScreen,
}

/// 失敗の見せ方（OS の生コード・メッセージは持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureView {
    /// 権限が無い。
    PermissionDenied,
    /// カメラが無い。
    NoCamera,
    /// 他で使用中。
    InUse,
    /// 開始できない（その他）。
    Other,
    /// 実行中に止まった。
    Runtime,
    /// 非対応。
    Unsupported,
}

/// ズーム失敗の見せ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomFailureView {
    /// この環境では使えない。
    Unsupported,
    /// 失敗。
    Failed,
}

/// ズームの段（device の `ZoomStep` を effect が写す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomStepView {
    /// 1 倍。
    One,
    /// 2 倍。
    Two,
}

/// 映像の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewPhase {
    /// 未開始。
    #[default]
    Idle,
    /// 開始要求が立ち、OS の開始通知待ち。
    Starting,
    /// 映像中（OS の開始通知で確定）。
    Live,
    /// OS が一時停止した。
    Interrupted,
    /// 停止（正常）。
    Stopped(StopView),
    /// 失敗・非対応。
    Failed(FailureView),
}

/// effect へ渡すズーム要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoomRequest {
    /// 開始要求の世代。
    pub generation: u64,
    /// 要求の通し番号（ドライバが二重実行を避けるのに使う）。
    pub seq: u64,
    /// 目標の段。
    pub step: ZoomStepView,
}

/// ズーム値の表示状態。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ZoomView {
    /// フレームがまだ届いていない。
    #[default]
    Unknown,
    /// OS から読んだ値。
    Value(f32),
    /// OS から読めない（この環境では使えない）。
    Unavailable,
}

/// 最新フレーム（縮小済み）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameView {
    /// 縮小後の画素。
    pub pixels: RgbaPixels,
    /// 元の幅。
    pub source_width: u32,
    /// 元の高さ。
    pub source_height: u32,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CameraPreviewStreamShowState {
    /// 進行状態。
    pub phase: PreviewPhase,
    /// 直近に立てた開始要求の世代。
    pub generation: u64,
    /// 最新フレーム（Live の間だけ持つ）。
    pub frame: Option<FrameView>,
    /// OS から届いたフレームの総数（今回の開始から）。
    pub frames_received: u64,
    /// ズーム値の表示。
    pub zoom: ZoomView,
    /// 未完了のズーム要求。
    pub zoom_request: Option<ZoomRequest>,
    /// ズーム要求の通し番号。
    pub zoom_seq: u64,
    /// ズームの案内（失敗時など）。
    pub zoom_status: Option<String>,
}

fn is_active(phase: PreviewPhase) -> bool {
    matches!(
        phase,
        PreviewPhase::Starting | PreviewPhase::Live | PreviewPhase::Interrupted
    )
}

/// 直前の映像に由来する表示を消す（直前の値を出し続けない）。
fn clear_media(state: &mut CameraPreviewStreamShowState) {
    state.frame = None;
    state.frames_received = 0;
    state.zoom = ZoomView::Unknown;
    state.zoom_request = None;
    state.zoom_status = None;
}

/// 開始ボタンのハンドラ。`Idle`・`Stopped`・`Failed` からだけ受け付け、前回の表示を消す。
pub fn start(state: &mut CameraPreviewStreamShowState) {
    if is_active(state.phase) {
        return;
    }
    state.phase = PreviewPhase::Starting;
    state.generation = state.generation.wrapping_add(1);
    clear_media(state);
}

/// 停止ボタンのハンドラ。利用者の停止は正常で、失敗にしない。
pub fn stop(state: &mut CameraPreviewStreamShowState) {
    if !is_active(state.phase) {
        return;
    }
    state.phase = PreviewPhase::Stopped(StopView::User);
    clear_media(state);
}

/// 開始 / 停止ボタンのハンドラ（ボタンは 1 つで、状態でラベルが変わる）。
pub fn toggle(state: &mut CameraPreviewStreamShowState) {
    if is_active(state.phase) {
        stop(state);
    } else {
        start(state);
    }
}

/// ズームを要求する。`Live` のときだけ受け付ける。
pub fn request_zoom(state: &mut CameraPreviewStreamShowState, step: ZoomStepView) {
    if state.phase != PreviewPhase::Live {
        return;
    }
    state.zoom_seq = state.zoom_seq.wrapping_add(1);
    state.zoom_status = None;
    state.zoom_request = Some(ZoomRequest {
        generation: state.generation,
        seq: state.zoom_seq,
        step,
    });
}

/// 「1 倍」ボタンのハンドラ。
pub fn zoom_one(state: &mut CameraPreviewStreamShowState) {
    request_zoom(state, ZoomStepView::One);
}

/// 「2 倍」ボタンのハンドラ。
pub fn zoom_two(state: &mut CameraPreviewStreamShowState) {
    request_zoom(state, ZoomStepView::Two);
}

/// 未処理の開始要求の世代を観測する（状態は変えない）。
pub fn take_start(state: &CameraPreviewStreamShowState) -> Option<u64> {
    (state.phase == PreviewPhase::Starting).then_some(state.generation)
}

/// 未処理のズーム要求を観測する（状態は変えない）。
pub fn take_zoom(state: &CameraPreviewStreamShowState) -> Option<ZoomRequest> {
    state.zoom_request
}

/// OS の開始通知（または中断からの再開）。`Starting`・`Interrupted` を `Live` にする。
pub fn apply_running(state: &mut CameraPreviewStreamShowState, generation: u64) {
    if state.generation != generation
        || !matches!(
            state.phase,
            PreviewPhase::Starting | PreviewPhase::Interrupted
        )
    {
        return;
    }
    state.phase = PreviewPhase::Live;
}

/// OS の中断通知。`Starting`・`Live` を `Interrupted` にし、古いフレームを映像として出さない。
pub fn apply_interrupted(state: &mut CameraPreviewStreamShowState, generation: u64) {
    if state.generation != generation
        || !matches!(state.phase, PreviewPhase::Starting | PreviewPhase::Live)
    {
        return;
    }
    state.phase = PreviewPhase::Interrupted;
    state.frame = None;
    state.zoom_request = None;
}

/// OS 側の停止（背景・画面を離れた）。
pub fn apply_stopped(state: &mut CameraPreviewStreamShowState, generation: u64, why: StopView) {
    if state.generation != generation || !is_active(state.phase) {
        return;
    }
    state.phase = PreviewPhase::Stopped(why);
    clear_media(state);
}

/// 開始の失敗。`Starting` の間だけ受ける。
pub fn apply_start_failed(
    state: &mut CameraPreviewStreamShowState,
    generation: u64,
    why: FailureView,
) {
    if state.generation != generation || state.phase != PreviewPhase::Starting {
        return;
    }
    state.phase = PreviewPhase::Failed(why);
    clear_media(state);
}

/// 実行中の失敗。`Live`・`Interrupted` の間だけ受ける。
pub fn apply_runtime_failed(state: &mut CameraPreviewStreamShowState, generation: u64) {
    if state.generation != generation
        || !matches!(state.phase, PreviewPhase::Live | PreviewPhase::Interrupted)
    {
        return;
    }
    state.phase = PreviewPhase::Failed(FailureView::Runtime);
    clear_media(state);
}

/// 非対応。`Starting` の間だけ受ける。
pub fn apply_unsupported(state: &mut CameraPreviewStreamShowState, generation: u64) {
    apply_start_failed(state, generation, FailureView::Unsupported);
}

/// フレームの到着。`Live` のときだけ受ける。`frames_received` は OS から届いた総数、`zoom` は
/// フレームと一緒に OS から読んだ値（`None` は「読めない」で `Unavailable`）。
pub fn apply_frame(
    state: &mut CameraPreviewStreamShowState,
    generation: u64,
    frames_received: u64,
    frame: FrameView,
    zoom: Option<f32>,
) {
    if state.generation != generation || state.phase != PreviewPhase::Live {
        return;
    }
    state.frames_received = frames_received;
    state.frame = Some(frame);
    state.zoom = match zoom {
        Some(z) => ZoomView::Value(z),
        None => ZoomView::Unavailable,
    };
}

/// ズーム要求の完了。要求を下ろし、失敗なら案内を出す。表示するズーム値は変えない。
pub fn finish_zoom(
    state: &mut CameraPreviewStreamShowState,
    generation: u64,
    seq: u64,
    result: Result<(), ZoomFailureView>,
) {
    if state.generation != generation || state.zoom_request.map(|r| r.seq) != Some(seq) {
        return;
    }
    state.zoom_request = None;
    state.zoom_status = result.err().map(zoom_failure_text);
}

/// OS からの非同期のズーム失敗。`Live` の間だけ案内を出す。
pub fn apply_zoom_failed(
    state: &mut CameraPreviewStreamShowState,
    generation: u64,
    why: ZoomFailureView,
) {
    if state.generation != generation || state.phase != PreviewPhase::Live {
        return;
    }
    state.zoom_status = Some(zoom_failure_text(why));
}

fn zoom_failure_text(why: ZoomFailureView) -> String {
    match why {
        ZoomFailureView::Unsupported => ZOOM_UNSUPPORTED_STATUS,
        ZoomFailureView::Failed => ZOOM_FAILED_STATUS,
    }
    .to_string()
}

fn phase_text(phase: PreviewPhase) -> String {
    match phase {
        PreviewPhase::Idle => IDLE_STATUS.to_string(),
        PreviewPhase::Starting => STARTING_STATUS.to_string(),
        PreviewPhase::Live => LIVE_STATUS.to_string(),
        PreviewPhase::Interrupted => INTERRUPTED_STATUS.to_string(),
        PreviewPhase::Stopped(StopView::User) => STOPPED_STATUS.to_string(),
        PreviewPhase::Stopped(StopView::Backgrounded) => BACKGROUNDED_STATUS.to_string(),
        PreviewPhase::Stopped(StopView::LeftScreen) => LEFT_SCREEN_STATUS.to_string(),
        PreviewPhase::Failed(FailureView::Runtime) => RUNTIME_FAILED_STATUS.to_string(),
        PreviewPhase::Failed(FailureView::Unsupported) => UNSUPPORTED_STATUS.to_string(),
        PreviewPhase::Failed(f) => {
            let reason = match f {
                FailureView::PermissionDenied => PERMISSION_DENIED_REASON,
                FailureView::NoCamera => NO_CAMERA_REASON,
                FailureView::InUse => IN_USE_REASON,
                _ => OTHER_REASON,
            };
            format!("{FAILED_PREFIX}{reason}")
        }
    }
}

/// 案内文言。`Live` のときはフレーム数・元寸法・ズーム値を足す。
pub fn status_text(state: &CameraPreviewStreamShowState) -> String {
    let mut lines = vec![phase_text(state.phase)];
    if state.phase == PreviewPhase::Live {
        match &state.frame {
            Some(f) => {
                lines.push(format!("{FRAMES_PREFIX}{}", state.frames_received));
                lines.push(format!(
                    "{SOURCE_PREFIX}{}×{}",
                    f.source_width, f.source_height
                ));
            }
            None => lines.push(NO_FRAME_TEXT.to_string()),
        }
        lines.push(match state.zoom {
            ZoomView::Unknown => format!("{ZOOM_PREFIX}-"),
            ZoomView::Value(z) => format!("{ZOOM_PREFIX}{z:.1} 倍"),
            ZoomView::Unavailable => format!("{ZOOM_PREFIX}{ZOOM_UNAVAILABLE_TEXT}"),
        });
        if let Some(s) = &state.zoom_status {
            lines.push(s.clone());
        }
    }
    lines.join("\n")
}

/// 映像ノード。`Live` で最新フレームがあるときだけ出す（描画のたびに評価される）。
fn expand_frame(state: &CameraPreviewStreamShowState) -> Vec<Node<CameraPreviewStreamShowState>> {
    match (&state.phase, &state.frame) {
        (PreviewPhase::Live, Some(f)) => vec![Node::RgbaImage {
            pixels: f.pixels.clone(),
            alt: FRAME_ALT.to_string(),
        }],
        _ => Vec::new(),
    }
}

/// 画面を組み立てる。開始 / 止めるは 1 つのボタンでラベルが変わり、ズームは `Live` のときだけ出す。
pub fn current_screen(
    state: &CameraPreviewStreamShowState,
) -> Screen<CameraPreviewStreamShowState> {
    let toggle_label = if is_active(state.phase) {
        STOP_LABEL
    } else {
        START_LABEL
    };
    let mut screen =
        Screen::new(TITLE)
            .dynamic_text(status_text)
            .button(TOGGLE_BUTTON_ID, toggle_label, toggle);
    if state.phase == PreviewPhase::Live {
        screen = screen
            .button(ZOOM_ONE_BUTTON_ID, ZOOM_ONE_LABEL, zoom_one)
            .button(ZOOM_TWO_BUTTON_ID, ZOOM_TWO_LABEL, zoom_two);
    }
    screen.list(expand_frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> FrameView {
        FrameView {
            pixels: RgbaPixels::new(1, 1, vec![1, 2, 3, 255]).unwrap(),
            source_width: 1280,
            source_height: 720,
        }
    }

    fn live() -> CameraPreviewStreamShowState {
        let mut s = CameraPreviewStreamShowState::default();
        start(&mut s);
        apply_running(&mut s, 1);
        s
    }

    #[test]
    fn startup_does_not_start() {
        let s = CameraPreviewStreamShowState::default();
        assert_eq!(take_start(&s), None);
        assert_eq!(s.phase, PreviewPhase::Idle);
    }

    #[test]
    fn stop_is_not_a_failure() {
        let mut s = live();
        stop(&mut s);
        assert_eq!(s.phase, PreviewPhase::Stopped(StopView::User));
        assert_eq!(status_text(&s), STOPPED_STATUS);
        assert!(!status_text(&s).contains(FAILED_PREFIX));
        // 開始待ちからの取り消しも同じ。
        let mut s = CameraPreviewStreamShowState::default();
        start(&mut s);
        stop(&mut s);
        assert_eq!(s.phase, PreviewPhase::Stopped(StopView::User));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = live();
        apply_stopped(&mut s, 99, StopView::Backgrounded);
        apply_interrupted(&mut s, 99);
        apply_runtime_failed(&mut s, 99);
        apply_frame(&mut s, 99, 5, frame(), Some(1.0));
        assert_eq!(s.phase, PreviewPhase::Live);
        assert!(s.frame.is_none());
        let mut s = CameraPreviewStreamShowState::default();
        start(&mut s);
        apply_start_failed(&mut s, 7, FailureView::Other);
        apply_running(&mut s, 7);
        assert_eq!(s.phase, PreviewPhase::Starting);
    }

    #[test]
    fn failures_and_interrupts_are_not_live() {
        let mut s = CameraPreviewStreamShowState::default();
        start(&mut s);
        apply_start_failed(&mut s, 1, FailureView::PermissionDenied);
        assert_eq!(s.phase, PreviewPhase::Failed(FailureView::PermissionDenied));
        assert_eq!(
            status_text(&s),
            format!("{FAILED_PREFIX}{PERMISSION_DENIED_REASON}")
        );

        let mut s = live();
        apply_interrupted(&mut s, 1);
        assert_eq!(s.phase, PreviewPhase::Interrupted);
        assert_eq!(status_text(&s), INTERRUPTED_STATUS);
        apply_running(&mut s, 1);
        assert_eq!(s.phase, PreviewPhase::Live);

        apply_runtime_failed(&mut s, 1);
        assert_eq!(s.phase, PreviewPhase::Failed(FailureView::Runtime));
        assert_eq!(status_text(&s), RUNTIME_FAILED_STATUS);
        // 失敗後にフレームが届いても映像にならない。
        apply_frame(&mut s, 1, 1, frame(), Some(1.0));
        assert!(s.frame.is_none());
        // 開始待ちの失敗は Live では受けない。
        let mut s = live();
        apply_start_failed(&mut s, 1, FailureView::Other);
        assert_eq!(s.phase, PreviewPhase::Live);
    }

    #[test]
    fn frames_only_in_live() {
        let mut s = CameraPreviewStreamShowState::default();
        start(&mut s);
        apply_frame(&mut s, 1, 1, frame(), Some(1.0));
        assert!(s.frame.is_none(), "Starting では受けない");
        apply_running(&mut s, 1);
        apply_frame(&mut s, 1, 3, frame(), Some(2.04));
        let t = status_text(&s);
        assert!(t.contains("受け取ったフレーム: 3"), "{t}");
        assert!(t.contains("元の寸法: 1280×720"), "{t}");
        assert!(t.contains("ズーム: 2.0 倍"), "{t}");
    }

    #[test]
    fn restart_clears_previous_values() {
        let mut s = live();
        apply_frame(&mut s, 1, 9, frame(), Some(2.0));
        stop(&mut s);
        assert!(s.frame.is_none());
        start(&mut s);
        assert_eq!(take_start(&s), Some(2));
        assert_eq!((s.frames_received, s.zoom), (0, ZoomView::Unknown));
        apply_running(&mut s, 2);
        assert!(status_text(&s).contains(NO_FRAME_TEXT));
        // 失敗後の再開始も同じ。
        apply_runtime_failed(&mut s, 2);
        apply_frame(&mut s, 2, 1, frame(), None);
        start(&mut s);
        assert!(s.frame.is_none());
    }

    #[test]
    fn double_start_is_ignored() {
        let mut s = CameraPreviewStreamShowState::default();
        start(&mut s);
        start(&mut s);
        assert_eq!(s.generation, 1);
        apply_running(&mut s, 1);
        start(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(s.phase, PreviewPhase::Live);
    }

    #[test]
    fn zoom_value_changes_only_by_os_value() {
        let mut s = live();
        apply_frame(&mut s, 1, 1, frame(), Some(1.0));
        zoom_two(&mut s);
        let req = take_zoom(&s).unwrap();
        assert_eq!(req.step, ZoomStepView::Two);
        assert_eq!(s.zoom, ZoomView::Value(1.0), "押下では変えない");
        finish_zoom(&mut s, req.generation, req.seq, Ok(()));
        assert_eq!(take_zoom(&s), None);
        assert_eq!(s.zoom, ZoomView::Value(1.0), "完了でも変えない");
        apply_frame(&mut s, 1, 2, frame(), Some(2.0));
        assert_eq!(s.zoom, ZoomView::Value(2.0));
    }

    #[test]
    fn zoom_requires_live_and_matching_seq() {
        let mut s = CameraPreviewStreamShowState::default();
        start(&mut s);
        zoom_one(&mut s);
        assert_eq!(take_zoom(&s), None);
        apply_running(&mut s, 1);
        zoom_one(&mut s);
        let req = take_zoom(&s).unwrap();
        finish_zoom(&mut s, 1, req.seq + 1, Ok(()));
        assert!(take_zoom(&s).is_some(), "別の seq は無視");
        finish_zoom(&mut s, 1, req.seq, Err(ZoomFailureView::Failed));
        assert_eq!(s.zoom_status.as_deref(), Some(ZOOM_FAILED_STATUS));
        assert!(status_text(&s).contains(ZOOM_FAILED_STATUS));
    }

    #[test]
    fn unavailable_zoom_is_shown() {
        let mut s = live();
        apply_frame(&mut s, 1, 1, frame(), None);
        assert!(status_text(&s).contains(ZOOM_UNAVAILABLE_TEXT));
    }

    #[test]
    fn unsupported_is_shown_and_never_live() {
        let mut s = CameraPreviewStreamShowState::default();
        start(&mut s);
        apply_unsupported(&mut s, 1);
        assert_eq!(status_text(&s), UNSUPPORTED_STATUS);
        assert_eq!(s.phase, PreviewPhase::Failed(FailureView::Unsupported));
    }

    #[test]
    fn os_stop_reasons() {
        let mut s = live();
        apply_stopped(&mut s, 1, StopView::Backgrounded);
        assert_eq!(status_text(&s), BACKGROUNDED_STATUS);
        let mut s = live();
        apply_stopped(&mut s, 1, StopView::LeftScreen);
        assert_eq!(status_text(&s), LEFT_SCREEN_STATUS);
    }

    #[test]
    fn buttons_follow_phase() {
        use crate::click_button;
        let mut s = CameraPreviewStreamShowState::default();
        assert!(click_button(&current_screen(&s), START_LABEL, &mut s));
        assert_eq!(s.phase, PreviewPhase::Starting);
        assert!(!click_button(&current_screen(&s), ZOOM_TWO_LABEL, &mut s));
        apply_running(&mut s, 1);
        assert!(click_button(&current_screen(&s), ZOOM_TWO_LABEL, &mut s));
        assert!(click_button(&current_screen(&s), STOP_LABEL, &mut s));
        assert_eq!(s.phase, PreviewPhase::Stopped(StopView::User));
    }
}
