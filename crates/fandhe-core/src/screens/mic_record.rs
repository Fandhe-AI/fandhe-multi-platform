//! Phase 11 の拡充ループ（親 #953）型サンプル「マイク録音」（Issue #954）。
//! 「録音開始」ボタン → 録音中（経過秒数を表示）→「停止」ボタンまたは
//! [`MAX_RECORD_MS`] 到達で自動停止 → 結果（サイズ・mime・長さ）または失敗理由・
//! Unsupported を表示する 1 画面。`FANDHE_PROXY_APP=mic_record` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod mic_record;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の録音開始・停止・権限確認は
//! 本モジュール外（`fandhe-egui-proxy` の effect ドライバ、`device/mic_record.rs`）が
//! [`take_start_request`]/[`take_stop_request`] で要求を観測し [`finish_record`] で
//! 結果を注入する（`biometric_check.rs`「認証した・キャンセル・使えない・失敗を
//! 混同しない理由」と同じ構造。マイク録音は権限確認・録音完了が非同期に届くため、
//! `fandhe-egui-proxy` 側は `MicRecorder` トレイトを通さず本状態機械へ直接書き戻す
//! 契約になる、`fandhe_device::mic_record` モジュール doc 参照）。
//!
//! # 経過秒数は画面側の時刻で表示する（OS からの継続 push は受けない）
//!
//! [`tick`] は effect ドライバが毎フレーム実測した経過ミリ秒（dt）を渡して呼ぶ
//! 純関数で、OS 側からの継続的な通知は前提にしない（`tilt_ball::step` と同じ
//! 「継続 push 型ではなく画面側で時間を進める」設計）。[`MAX_RECORD_MS`] に達したら
//! 本関数自身が [`RecordPhase::StopRequested`] へ進め、明示的な「停止」ボタン操作と
//! 同じ経路で終了させる（Rust 側と OS 側（ネイティブの `recordForDuration:`）の
//! 期限を [`MAX_RECORD_MS`] という単一の定数で揃える、`device-bridge-contracts.md`
//! 「期限・キャンセル」節参照）。
//!
//! ```text
//! Idle --「録音開始」--> Requested, generation += 1
//!   --effect が take_start_request で観測 → mark_recording--> Recording
//!   --tick（毎フレーム）--> Recording（elapsed_ms 更新。上限到達で自動的に StopRequested へ）
//!   --「停止」--> StopRequested
//!   --effect が take_stop_request で観測 → mark_stopping--> Stopping
//!   --finish_record（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```
//!
//! 世代番号（`generation`）は [`take_start_request`]/[`finish_record`] の照合に使う
//! （`biometric_check::BiometricCheckState` と同じ判断）。停止操作
//! （[`request_stop`]/[`take_stop_request`]/[`mark_stopping`]）は世代を進めない
//! （同じ録音セッションの継続のため）。

use crate::{Screen, WidgetId};

/// 「録音開始」ボタンの `WidgetId`。
const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「停止」ボタンの `WidgetId`。
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 録音時間の上限（ミリ秒）。ネイティブ側（macOS `recordForDuration:`）と Rust 側
/// （[`tick`] の自動停止）の期限をこの 1 定数で揃える（モジュール doc 参照）。
pub const MAX_RECORD_MS: u64 = 30_000;

/// 待機中の案内。
pub const IDLE_STATUS: &str = "待機";
/// 権限確認中に利用者がキャンセルした際の案内（`biometric_check::CANCELLED_STATUS`
/// と同型。キャンセルは正常な利用者操作であり失敗ではないため、[`FAILED_PREFIX`]
/// を付けた `error` ではなく通常色の `status` へ載せる、Issue #954/#959 レビュー
/// 指摘「Cancel shown as recording failure」対応）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 内部エラー・Unsupported 時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 録音操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// 「録音開始」ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが録音を開始した（[`mark_recording`] 済み）。
    Recording,
    /// 「停止」ボタンが押された、または [`tick`] が上限到達で自動的に遷移させた。
    /// effect ドライバへまだ渡していない。
    StopRequested,
    /// effect ドライバが停止を依頼し、結果を待っている（[`mark_stopping`] 済み）。
    Stopping,
}

/// 録音結果の要約（画面表示用。生バイト列は保持しない。
/// `fandhe_device::mic_record::Recording` からの変換は `fandhe-egui-proxy` 側の
/// 責務にする、`fandhe-core` は `fandhe-device` に依存しないため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordSummary {
    /// 録音データのサイズ（バイト）。
    pub size_bytes: usize,
    /// MIME タイプ（例: `"audio/mp4"`）。
    pub mime: String,
    /// 録音時間（ミリ秒）。
    pub duration_ms: u64,
}

/// [`finish_record`] へ渡す正常系の結果（`biometric_check::AuthOutcome` と同型）。
/// 「録音できた」と「利用者が権限確認中にキャンセルした」（正常な操作）を区別し、
/// 後者を [`FAILED_PREFIX`] 付きの `error` へ畳まないための型（Issue #954/#959
/// レビュー指摘「Cancel shown as recording failure」対応）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// 録音できた。
    Recorded(RecordSummary),
    /// 権限確認中に利用者がキャンセルした。
    Cancelled,
}

/// effect ドライバへ渡す開始要求（[`take_start_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartRequest {
    /// [`finish_record`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 録音時間の上限（ミリ秒、常に [`MAX_RECORD_MS`]）。
    pub max_duration_ms: u64,
}

/// effect ドライバへ渡す停止要求（[`take_stop_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopRequest {
    /// [`finish_record`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// マイク録音画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MicRecordState {
    /// 録音操作の進行状態。
    pub phase: RecordPhase,
    /// 直近に立てた開始要求の世代番号（モジュール doc 参照）。
    pub generation: u64,
    /// 経過時間（ミリ秒）。[`tick`] のみが更新する。[`MAX_RECORD_MS`] を超えない。
    pub elapsed_ms: u64,
    /// 直近の結果案内文言（通常色で表示。`Idle` かつ未操作なら空文字列）。
    pub status: String,
    /// 内部エラー・Unsupported 時の案内（`TextRole::Error` で表示）。正常系は
    /// 空文字列にする（成功と失敗を取り違えないための分離、`biometric_check` と同型）。
    pub error: String,
}

/// 「録音開始」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op
/// （panic を API 契約にしない、coding-rust.md）。
pub fn start_recording(state: &mut MicRecordState) {
    if state.phase != RecordPhase::Idle {
        return;
    }
    state.phase = RecordPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.elapsed_ms = 0;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の開始要求を観測するための getter（状態は変えない。
/// 実際に `Recording` へ進めるのは [`mark_recording`]）。
pub fn take_start_request(state: &MicRecordState) -> Option<StartRequest> {
    (state.phase == RecordPhase::Requested).then_some(StartRequest {
        generation: state.generation,
        max_duration_ms: MAX_RECORD_MS,
    })
}

/// 開始要求を録音中（`Recording`）へ進める。`Requested` 以外では何もしない。
pub fn mark_recording(state: &mut MicRecordState) {
    if state.phase == RecordPhase::Requested {
        state.phase = RecordPhase::Recording;
    }
}

/// 経過時間を進める（`Recording` のときのみ、モジュール doc 参照）。`dt_ms` だけ
/// `elapsed_ms` を進め、[`MAX_RECORD_MS`] で頭打ちにする。上限へ達したら
/// 「停止」ボタンを押したのと同じ [`RecordPhase::StopRequested`] へ自動的に進める。
pub fn tick(state: &mut MicRecordState, dt_ms: u64) {
    if state.phase != RecordPhase::Recording {
        return;
    }
    state.elapsed_ms = (state.elapsed_ms + dt_ms).min(MAX_RECORD_MS);
    if state.elapsed_ms >= MAX_RECORD_MS {
        state.phase = RecordPhase::StopRequested;
    }
}

/// 「停止」ボタンのハンドラ。`Recording` 以外は no-op（二重停止を防ぐ）。
pub fn request_stop(state: &mut MicRecordState) {
    if state.phase == RecordPhase::Recording {
        state.phase = RecordPhase::StopRequested;
    }
}

/// effect ドライバが未発送の停止要求を観測するための getter（状態は変えない）。
pub fn take_stop_request(state: &MicRecordState) -> Option<StopRequest> {
    (state.phase == RecordPhase::StopRequested).then_some(StopRequest {
        generation: state.generation,
    })
}

/// 停止要求を結果待ち（`Stopping`）へ進める。`StopRequested` 以外では何もしない。
pub fn mark_stopping(state: &mut MicRecordState) {
    if state.phase == RecordPhase::StopRequested {
        state.phase = RecordPhase::Stopping;
    }
}

/// 録音結果を反映する。`generation` は [`take_start_request`] が返した世代番号
/// （呼び出し元が録音中も保持し続ける契約）。現在の世代と一致しない完了、
/// `Idle`（要求が無い）状態で届いた完了は無視する（`biometric_check::finish_auth`
/// と同型）。
pub fn finish_record(
    state: &mut MicRecordState,
    generation: u64,
    result: Result<RecordOutcome, String>,
) {
    if state.phase == RecordPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = RecordPhase::Idle;
    match result {
        Ok(RecordOutcome::Recorded(summary)) => {
            state.status = format!(
                "結果: {} バイト・{}・{} ミリ秒",
                summary.size_bytes, summary.mime, summary.duration_ms
            );
            state.error = String::new();
        }
        Ok(RecordOutcome::Cancelled) => {
            state.status = CANCELLED_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 案内文言（`DynamicText` の `get`）。録音中は経過秒数、それ以外は直近の結果
/// （未操作なら [`IDLE_STATUS`]）を表示する。
pub fn status_text(state: &MicRecordState) -> String {
    match state.phase {
        RecordPhase::Idle => {
            if state.status.is_empty() {
                IDLE_STATUS.to_string()
            } else {
                state.status.clone()
            }
        }
        _ => format!("録音中 {}秒", state.elapsed_ms / 1000),
    }
}

/// 内部エラー・Unsupported 時の案内文言（`error_text` の `get`。`TextRole::Error`
/// として描画される）。
pub fn error_text(state: &MicRecordState) -> String {
    state.error.clone()
}

/// マイク録音画面を組み立てる。`registry.rs` の `effect` 区画から `fn(&S) ->
/// Screen<S>` として呼ばれる契約。`Idle` は「録音開始」、`Recording` は「停止」を
/// 出し、それ以外（`Requested`/`StopRequested`/`Stopping`）は二重押しを防ぐため
/// ボタンを外す（`biometric_check::current_screen` と同型）。
pub fn current_screen(state: &MicRecordState) -> Screen<MicRecordState> {
    let screen = Screen::new("マイク録音")
        .dynamic_text(status_text)
        .error_text(error_text);
    match state.phase {
        RecordPhase::Idle => screen.button(START_BUTTON_ID, "録音開始", start_recording),
        RecordPhase::Recording => screen.button(STOP_BUTTON_ID, "停止", request_stop),
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_recording_advances_generation_and_phase() {
        let mut state = MicRecordState::default();
        start_recording(&mut state);
        assert_eq!(state.phase, RecordPhase::Requested);
        assert_eq!(state.generation, 1);
        assert_eq!(state.elapsed_ms, 0);
    }

    #[test]
    fn start_recording_is_noop_while_in_flight() {
        let mut state = MicRecordState {
            phase: RecordPhase::Recording,
            generation: 1,
            elapsed_ms: 5_000,
            ..Default::default()
        };
        start_recording(&mut state);
        assert_eq!(state.phase, RecordPhase::Recording);
        assert_eq!(state.generation, 1);
        assert_eq!(state.elapsed_ms, 5_000);
    }

    #[test]
    fn start_recording_clears_previous_status_and_error() {
        let mut state = MicRecordState {
            status: "前回の結果".to_string(),
            error: format!("{FAILED_PREFIX}前回の失敗"),
            ..Default::default()
        };
        start_recording(&mut state);
        assert_eq!(state.status, "");
        assert_eq!(state.error, "");
    }

    #[test]
    fn take_start_request_returns_generation_only_when_requested() {
        let mut state = MicRecordState::default();
        assert_eq!(take_start_request(&state), None);
        start_recording(&mut state);
        assert_eq!(
            take_start_request(&state),
            Some(StartRequest {
                generation: 1,
                max_duration_ms: MAX_RECORD_MS,
            })
        );
        mark_recording(&mut state);
        assert_eq!(take_start_request(&state), None);
    }

    #[test]
    fn tick_accumulates_elapsed_ms_while_recording() {
        let mut state = MicRecordState {
            phase: RecordPhase::Recording,
            ..Default::default()
        };
        tick(&mut state, 500);
        tick(&mut state, 250);
        assert_eq!(state.elapsed_ms, 750);
        assert_eq!(state.phase, RecordPhase::Recording);
    }

    #[test]
    fn tick_is_noop_outside_recording() {
        let mut state = MicRecordState {
            phase: RecordPhase::Idle,
            ..Default::default()
        };
        tick(&mut state, 500);
        assert_eq!(state.elapsed_ms, 0);
    }

    #[test]
    fn tick_caps_at_max_and_auto_requests_stop() {
        let mut state = MicRecordState {
            phase: RecordPhase::Recording,
            elapsed_ms: MAX_RECORD_MS - 100,
            ..Default::default()
        };
        tick(&mut state, 1_000);
        assert_eq!(state.elapsed_ms, MAX_RECORD_MS);
        assert_eq!(state.phase, RecordPhase::StopRequested);
    }

    #[test]
    fn request_stop_only_transitions_from_recording() {
        let mut idle = MicRecordState::default();
        request_stop(&mut idle);
        assert_eq!(idle.phase, RecordPhase::Idle);

        let mut recording = MicRecordState {
            phase: RecordPhase::Recording,
            ..Default::default()
        };
        request_stop(&mut recording);
        assert_eq!(recording.phase, RecordPhase::StopRequested);
    }

    #[test]
    fn take_stop_request_returns_generation_only_when_stop_requested() {
        let mut state = MicRecordState {
            phase: RecordPhase::Recording,
            generation: 3,
            ..Default::default()
        };
        assert_eq!(take_stop_request(&state), None);
        request_stop(&mut state);
        assert_eq!(
            take_stop_request(&state),
            Some(StopRequest { generation: 3 })
        );
        mark_stopping(&mut state);
        assert_eq!(take_stop_request(&state), None);
    }

    #[test]
    fn finish_record_success_sets_status_without_error() {
        let mut state = MicRecordState::default();
        start_recording(&mut state);
        mark_recording(&mut state);
        request_stop(&mut state);
        mark_stopping(&mut state);
        finish_record(
            &mut state,
            1,
            Ok(RecordOutcome::Recorded(RecordSummary {
                size_bytes: 12_345,
                mime: "audio/mp4".to_string(),
                duration_ms: 1_500,
            })),
        );
        assert_eq!(state.phase, RecordPhase::Idle);
        assert!(state.status.contains("12345"));
        assert!(state.status.contains("audio/mp4"));
        assert!(state.status.contains("1500"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_record_error_sets_error_without_success_status() {
        let mut state = MicRecordState::default();
        start_recording(&mut state);
        mark_recording(&mut state);
        finish_record(
            &mut state,
            1,
            Err("この環境ではマイク録音を使えません".to_string()),
        );
        assert_eq!(state.phase, RecordPhase::Idle);
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
        assert!(state.error.contains("使えません"));
    }

    #[test]
    fn finish_record_ignores_stale_generation() {
        let mut state = MicRecordState::default();
        start_recording(&mut state); // generation = 1
        mark_recording(&mut state);
        request_stop(&mut state);
        mark_stopping(&mut state);
        finish_record(&mut state, 1, Err("失敗".to_string())); // Idle に戻る

        start_recording(&mut state); // generation = 2
        finish_record(
            &mut state,
            1,
            Ok(RecordOutcome::Recorded(RecordSummary {
                size_bytes: 1,
                mime: "audio/mp4".to_string(),
                duration_ms: 1,
            })),
        );
        // 古い世代の完了は無視され、Requested のまま
        assert_eq!(state.phase, RecordPhase::Requested);
        assert_eq!(state.status, "");
    }

    #[test]
    fn finish_record_ignores_completion_when_idle() {
        let mut state = MicRecordState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_record(
            &mut state,
            0,
            Ok(RecordOutcome::Recorded(RecordSummary {
                size_bytes: 1,
                mime: "audio/mp4".to_string(),
                duration_ms: 1,
            })),
        );
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn finish_record_cancelled_sets_status_without_error() {
        // 権限確認中のキャンセルは正常な利用者操作であり、`FAILED_PREFIX` 付きの
        // `error` ではなく通常色の `status` へ載せる（Issue #954/#959 レビュー
        // 指摘「Cancel shown as recording failure」対応）。
        let mut state = MicRecordState::default();
        start_recording(&mut state);
        mark_recording(&mut state);
        finish_record(&mut state, 1, Ok(RecordOutcome::Cancelled));
        assert_eq!(state.phase, RecordPhase::Idle);
        assert_eq!(state.status, CANCELLED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn status_text_shows_idle_recording_and_result() {
        let idle = MicRecordState::default();
        assert_eq!(status_text(&idle), IDLE_STATUS);

        let recording = MicRecordState {
            phase: RecordPhase::Recording,
            elapsed_ms: 4_200,
            ..Default::default()
        };
        assert_eq!(status_text(&recording), "録音中 4秒");

        let done = MicRecordState {
            status: "結果: 100 バイト・audio/mp4・1000 ミリ秒".to_string(),
            ..Default::default()
        };
        assert_eq!(status_text(&done), done.status);
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = MicRecordState {
            phase: RecordPhase::Stopping,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "録音開始", &mut state));
        assert!(!click_button(&screen, "停止", &mut state));
    }

    #[test]
    fn start_button_is_clickable_when_idle() {
        use crate::click_button;
        let mut state = MicRecordState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "録音開始", &mut state));
        assert_eq!(state.phase, RecordPhase::Requested);
    }

    #[test]
    fn stop_button_is_clickable_while_recording() {
        use crate::click_button;
        let mut state = MicRecordState {
            phase: RecordPhase::Recording,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(click_button(&screen, "停止", &mut state));
        assert_eq!(state.phase, RecordPhase::StopRequested);
    }
}
