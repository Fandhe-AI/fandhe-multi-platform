//! Phase 11 の拡充ループ（親 #1023）型サンプル「音声認識」（Issue #1024）。
//! 「話す」ボタン → 聞き取り中（経過秒数を表示）→「停止」ボタンまたは
//! [`MAX_LISTEN_MS`] 到達で自動停止 → 認識結果の文字列・「聞き取れませんでした」・
//! 「キャンセルしました」・失敗理由・Unsupported を表示する 1 画面。
//! `FANDHE_PROXY_APP=speech_recognize` で選ばれ、`crates/fandhe-egui-proxy/src/
//! screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の
//! `pub mod speech_recognize;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の聞き取り開始・停止・権限確認は
//! 本モジュール外（`fandhe-egui-proxy` の effect ドライバ、`device/
//! speech_recognition.rs`）が [`take_start_request`]/[`take_stop_request`] で要求を
//! 観測し [`finish_listen`] で結果を注入する（`mic_record.rs`「端末依存はゼロ」節と
//! 同じ構造。macOS/Windows は認識開始・完了が非同期に届くため、`fandhe-egui-proxy`
//! 側は `SpeechRecognizer` トレイトを通さず本状態機械へ直接書き戻す契約になる、
//! `fandhe_device::speech_recognition` モジュール doc 参照）。
//!
//! # 経過秒数は画面側の時刻で表示する（OS からの継続 push は受けない）
//!
//! [`tick`] は effect ドライバが毎フレーム実測した経過ミリ秒（dt）を渡して呼ぶ
//! 純関数で、OS 側からの継続的な通知は前提にしない（`mic_record::tick` と同じ
//! 「継続 push 型ではなく画面側で時間を進める」設計）。[`MAX_LISTEN_MS`] に達したら
//! 本関数自身が [`ListenPhase::StopRequested`] へ進め、明示的な「停止」ボタン操作と
//! 同じ経路で終了させる（Rust 側と OS 側（macOS の `dispatch_after`、Windows の
//! `recv_timeout`）の期限を [`MAX_LISTEN_MS`] という単一の定数で揃える、
//! `device-bridge-contracts.md` 「期限・キャンセル」節参照）。
//!
//! # 「停止」ボタンの意味（[`ListenOutcome`] の三値）
//!
//! 「停止」は「話し終わった」の合図として OS に結果の確定を求める。確定した文字列が
//! あれば [`ListenOutcome::Recognized`]、無ければ [`ListenOutcome::NoMatch`] になる。
//! 権限確認中など、まだ聞き始めていない段階の停止・OS 側の利用者キャンセルは
//! [`ListenOutcome::Cancelled`]（正常な利用者操作であり失敗ではないため、
//! [`FAILED_PREFIX`] を付けた `error` ではなく通常色の `status` へ載せる、
//! `mic_record.rs::RecordOutcome::Cancelled` と同型）。停止を失敗として見せる経路は
//! 作らない（`fandhe_device::speech_recognition` モジュール doc 参照）。
//!
//! ```text
//! Idle --「話す」--> Requested, generation += 1
//!   --effect が take_start_request で観測 → mark_listening--> Listening
//!   --tick（毎フレーム）--> Listening（elapsed_ms 更新。上限到達で自動的に StopRequested へ）
//!   --「停止」--> StopRequested
//!   --effect が take_stop_request で観測 → mark_stopping--> Stopping
//!   --finish_listen（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```
//!
//! 世代番号（`generation`）は [`take_start_request`]/[`finish_listen`] の照合に使う
//! （`mic_record::MicRecordState` と同じ判断）。停止操作
//! （[`request_stop`]/[`take_stop_request`]/[`mark_stopping`]）は世代を進めない
//! （同じ聞き取りセッションの継続のため）。

use crate::{Screen, WidgetId};

/// 「話す」ボタンの `WidgetId`。
const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「停止」ボタンの `WidgetId`。
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 聞き取りの上限（ミリ秒）。`fandhe_device::mic_record::MAX_RECORDING_BYTES` とは
/// 別軸の時間上限で、`mic_record::MAX_RECORD_MS`（ネイティブ側 `recordForDuration:`
/// 相当の上限）と単一の定数で揃える SSOT（モジュール doc 参照）。macOS 側
/// （`dispatch_after`）・Windows 側（ワーカーの `recv_timeout`）の期限もこの値を使う。
pub const MAX_LISTEN_MS: u64 = super::mic_record::MAX_RECORD_MS;

/// 待機中の案内。
pub const IDLE_STATUS: &str = "待機";
/// 聞き取れなかった（無音・結果 0 件・空文字・OS の無音タイムアウト）際の案内。
pub const NO_MATCH_STATUS: &str = "聞き取れませんでした";
/// 権限確認中・OS 側の利用者キャンセルの際の案内（`mic_record::CANCELLED_STATUS`
/// と同型。キャンセルは正常な利用者操作であり失敗ではないため、[`FAILED_PREFIX`]
/// を付けた `error` ではなく通常色の `status` へ載せる）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 内部エラー・Unsupported 時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 聞き取り操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListenPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// 「話す」ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが聞き取りを開始した（[`mark_listening`] 済み）。
    Listening,
    /// 「停止」ボタンが押された、または [`tick`] が上限到達で自動的に遷移させた。
    /// effect ドライバへまだ渡していない。
    StopRequested,
    /// effect ドライバが停止を依頼し、結果を待っている（[`mark_stopping`] 済み）。
    Stopping,
}

/// [`finish_listen`] へ渡す正常系の結果（`mic_record::RecordOutcome` と同型。
/// 「認識できた」「聞き取れなかった」「利用者がキャンセルした」を区別する、
/// モジュール doc の三値参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenOutcome {
    /// 認識できた文字列（空白以外を含む。trim しない）。
    Recognized(String),
    /// 無音・結果 0 件・空文字・OS の無音タイムアウト。
    NoMatch,
    /// 権限確認中（開始前）の停止、または OS 側の利用者キャンセル。
    Cancelled,
}

/// effect ドライバへ渡す開始要求（[`take_start_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartRequest {
    /// [`finish_listen`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 聞き取りの上限（ミリ秒、常に [`MAX_LISTEN_MS`]）。
    pub max_listen_ms: u64,
}

/// effect ドライバへ渡す停止要求（[`take_stop_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopRequest {
    /// [`finish_listen`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 音声認識画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpeechRecognizeState {
    /// 聞き取り操作の進行状態。
    pub phase: ListenPhase,
    /// 直近に立てた開始要求の世代番号（モジュール doc 参照）。
    pub generation: u64,
    /// 経過時間（ミリ秒）。[`tick`] のみが更新する。[`MAX_LISTEN_MS`] を超えない。
    pub elapsed_ms: u64,
    /// 直近の結果案内文言（通常色で表示。`Idle` かつ未操作なら空文字列）。
    pub status: String,
    /// 内部エラー・Unsupported 時の案内（`TextRole::Error` で表示）。正常系は
    /// 空文字列にする（成功と失敗を取り違えないための分離、`mic_record` と同型）。
    pub error: String,
}

/// 「話す」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op
/// （panic を API 契約にしない、coding-rust.md）。
pub fn start_listening(state: &mut SpeechRecognizeState) {
    if state.phase != ListenPhase::Idle {
        return;
    }
    state.phase = ListenPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.elapsed_ms = 0;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の開始要求を観測するための getter（状態は変えない。
/// 実際に `Listening` へ進めるのは [`mark_listening`]）。
pub fn take_start_request(state: &SpeechRecognizeState) -> Option<StartRequest> {
    (state.phase == ListenPhase::Requested).then_some(StartRequest {
        generation: state.generation,
        max_listen_ms: MAX_LISTEN_MS,
    })
}

/// 開始要求を聞き取り中（`Listening`）へ進める。`Requested` 以外では何もしない。
pub fn mark_listening(state: &mut SpeechRecognizeState) {
    if state.phase == ListenPhase::Requested {
        state.phase = ListenPhase::Listening;
    }
}

/// 経過時間を進める（`Listening` のときのみ、モジュール doc 参照）。`dt_ms` だけ
/// `elapsed_ms` を進め、[`MAX_LISTEN_MS`] で頭打ちにする。上限へ達したら
/// 「停止」ボタンを押したのと同じ [`ListenPhase::StopRequested`] へ自動的に進める。
pub fn tick(state: &mut SpeechRecognizeState, dt_ms: u64) {
    if state.phase != ListenPhase::Listening {
        return;
    }
    state.elapsed_ms = (state.elapsed_ms + dt_ms).min(MAX_LISTEN_MS);
    if state.elapsed_ms >= MAX_LISTEN_MS {
        state.phase = ListenPhase::StopRequested;
    }
}

/// 「停止」ボタンのハンドラ。`Listening` 以外は no-op（二重停止を防ぐ）。
pub fn request_stop(state: &mut SpeechRecognizeState) {
    if state.phase == ListenPhase::Listening {
        state.phase = ListenPhase::StopRequested;
    }
}

/// effect ドライバが未発送の停止要求を観測するための getter（状態は変えない）。
pub fn take_stop_request(state: &SpeechRecognizeState) -> Option<StopRequest> {
    (state.phase == ListenPhase::StopRequested).then_some(StopRequest {
        generation: state.generation,
    })
}

/// 停止要求を結果待ち（`Stopping`）へ進める。`StopRequested` 以外では何もしない。
pub fn mark_stopping(state: &mut SpeechRecognizeState) {
    if state.phase == ListenPhase::StopRequested {
        state.phase = ListenPhase::Stopping;
    }
}

/// 認識結果を反映する。`generation` は [`take_start_request`] が返した世代番号
/// （呼び出し元が聞き取り中も保持し続ける契約）。現在の世代と一致しない完了、
/// `Idle`（要求が無い）状態で届いた完了は無視する（`mic_record::finish_record`
/// と同型）。
pub fn finish_listen(
    state: &mut SpeechRecognizeState,
    generation: u64,
    result: Result<ListenOutcome, String>,
) {
    if state.phase == ListenPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ListenPhase::Idle;
    match result {
        Ok(ListenOutcome::Recognized(text)) => {
            state.status = format!("認識結果: {text}");
            state.error = String::new();
        }
        Ok(ListenOutcome::NoMatch) => {
            state.status = NO_MATCH_STATUS.to_string();
            state.error = String::new();
        }
        Ok(ListenOutcome::Cancelled) => {
            state.status = CANCELLED_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 案内文言（`DynamicText` の `get`）。聞き取り中は経過秒数、それ以外は直近の結果
/// （未操作なら [`IDLE_STATUS`]）を表示する。
pub fn status_text(state: &SpeechRecognizeState) -> String {
    match state.phase {
        ListenPhase::Idle => {
            if state.status.is_empty() {
                IDLE_STATUS.to_string()
            } else {
                state.status.clone()
            }
        }
        _ => format!("聞き取り中 {}秒", state.elapsed_ms / 1000),
    }
}

/// 内部エラー・Unsupported 時の案内文言（`error_text` の `get`。`TextRole::Error`
/// として描画される）。
pub fn error_text(state: &SpeechRecognizeState) -> String {
    state.error.clone()
}

/// 音声認識画面を組み立てる。`registry.rs` の `effect` 区画から `fn(&S) ->
/// Screen<S>` として呼ばれる契約。`Idle` は「話す」、`Listening` は「停止」を出し、
/// それ以外（`Requested`/`StopRequested`/`Stopping`）は二重押しを防ぐためボタンを
/// 外す（`mic_record::current_screen` と同型）。
pub fn current_screen(state: &SpeechRecognizeState) -> Screen<SpeechRecognizeState> {
    let screen = Screen::new("音声認識")
        .dynamic_text(status_text)
        .error_text(error_text);
    match state.phase {
        ListenPhase::Idle => screen.button(START_BUTTON_ID, "話す", start_listening),
        ListenPhase::Listening => screen.button(STOP_BUTTON_ID, "停止", request_stop),
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_listen_ms_matches_mic_record_ssot() {
        assert_eq!(MAX_LISTEN_MS, super::super::mic_record::MAX_RECORD_MS);
    }

    #[test]
    fn start_listening_advances_generation_and_phase() {
        let mut state = SpeechRecognizeState::default();
        start_listening(&mut state);
        assert_eq!(state.phase, ListenPhase::Requested);
        assert_eq!(state.generation, 1);
        assert_eq!(state.elapsed_ms, 0);
    }

    #[test]
    fn start_listening_is_noop_while_in_flight() {
        let mut state = SpeechRecognizeState {
            phase: ListenPhase::Listening,
            generation: 1,
            elapsed_ms: 5_000,
            ..Default::default()
        };
        start_listening(&mut state);
        assert_eq!(state.phase, ListenPhase::Listening);
        assert_eq!(state.generation, 1);
        assert_eq!(state.elapsed_ms, 5_000);
    }

    #[test]
    fn start_listening_clears_previous_status_and_error() {
        let mut state = SpeechRecognizeState {
            status: "前回の結果".to_string(),
            error: format!("{FAILED_PREFIX}前回の失敗"),
            ..Default::default()
        };
        start_listening(&mut state);
        assert_eq!(state.status, "");
        assert_eq!(state.error, "");
    }

    #[test]
    fn take_start_request_returns_generation_only_when_requested() {
        let mut state = SpeechRecognizeState::default();
        assert_eq!(take_start_request(&state), None);
        start_listening(&mut state);
        assert_eq!(
            take_start_request(&state),
            Some(StartRequest {
                generation: 1,
                max_listen_ms: MAX_LISTEN_MS,
            })
        );
        mark_listening(&mut state);
        assert_eq!(take_start_request(&state), None);
    }

    #[test]
    fn tick_accumulates_elapsed_ms_while_listening() {
        let mut state = SpeechRecognizeState {
            phase: ListenPhase::Listening,
            ..Default::default()
        };
        tick(&mut state, 500);
        tick(&mut state, 250);
        assert_eq!(state.elapsed_ms, 750);
        assert_eq!(state.phase, ListenPhase::Listening);
    }

    #[test]
    fn tick_is_noop_outside_listening() {
        let mut state = SpeechRecognizeState {
            phase: ListenPhase::Idle,
            ..Default::default()
        };
        tick(&mut state, 500);
        assert_eq!(state.elapsed_ms, 0);
    }

    #[test]
    fn tick_caps_at_max_and_auto_requests_stop() {
        let mut state = SpeechRecognizeState {
            phase: ListenPhase::Listening,
            elapsed_ms: MAX_LISTEN_MS - 100,
            ..Default::default()
        };
        tick(&mut state, 1_000);
        assert_eq!(state.elapsed_ms, MAX_LISTEN_MS);
        assert_eq!(state.phase, ListenPhase::StopRequested);
    }

    #[test]
    fn request_stop_only_transitions_from_listening() {
        let mut idle = SpeechRecognizeState::default();
        request_stop(&mut idle);
        assert_eq!(idle.phase, ListenPhase::Idle);

        let mut listening = SpeechRecognizeState {
            phase: ListenPhase::Listening,
            ..Default::default()
        };
        request_stop(&mut listening);
        assert_eq!(listening.phase, ListenPhase::StopRequested);
    }

    #[test]
    fn take_stop_request_returns_generation_only_when_stop_requested() {
        let mut state = SpeechRecognizeState {
            phase: ListenPhase::Listening,
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
    fn finish_listen_recognized_sets_status_without_error() {
        let mut state = SpeechRecognizeState::default();
        start_listening(&mut state);
        mark_listening(&mut state);
        request_stop(&mut state);
        mark_stopping(&mut state);
        finish_listen(
            &mut state,
            1,
            Ok(ListenOutcome::Recognized("こんにちは".to_string())),
        );
        assert_eq!(state.phase, ListenPhase::Idle);
        assert_eq!(state.status, "認識結果: こんにちは");
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_listen_no_match_sets_status_without_error() {
        let mut state = SpeechRecognizeState::default();
        start_listening(&mut state);
        mark_listening(&mut state);
        finish_listen(&mut state, 1, Ok(ListenOutcome::NoMatch));
        assert_eq!(state.phase, ListenPhase::Idle);
        assert_eq!(state.status, NO_MATCH_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_listen_cancelled_sets_status_without_error() {
        let mut state = SpeechRecognizeState::default();
        start_listening(&mut state);
        mark_listening(&mut state);
        finish_listen(&mut state, 1, Ok(ListenOutcome::Cancelled));
        assert_eq!(state.phase, ListenPhase::Idle);
        assert_eq!(state.status, CANCELLED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_listen_error_sets_error_without_success_status() {
        let mut state = SpeechRecognizeState::default();
        start_listening(&mut state);
        mark_listening(&mut state);
        finish_listen(
            &mut state,
            1,
            Err("この環境では音声認識を使えません".to_string()),
        );
        assert_eq!(state.phase, ListenPhase::Idle);
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
        assert!(state.error.contains("使えません"));
    }

    #[test]
    fn finish_listen_ignores_stale_generation() {
        let mut state = SpeechRecognizeState::default();
        start_listening(&mut state); // generation = 1
        mark_listening(&mut state);
        request_stop(&mut state);
        mark_stopping(&mut state);
        finish_listen(&mut state, 1, Err("失敗".to_string())); // Idle に戻る

        start_listening(&mut state); // generation = 2
        finish_listen(
            &mut state,
            1,
            Ok(ListenOutcome::Recognized("古い結果".to_string())),
        );
        // 古い世代の完了は無視され、Requested のまま
        assert_eq!(state.phase, ListenPhase::Requested);
        assert_eq!(state.status, "");
    }

    #[test]
    fn finish_listen_ignores_completion_when_idle() {
        let mut state = SpeechRecognizeState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_listen(
            &mut state,
            0,
            Ok(ListenOutcome::Recognized("x".to_string())),
        );
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn status_text_shows_idle_listening_and_result() {
        let idle = SpeechRecognizeState::default();
        assert_eq!(status_text(&idle), IDLE_STATUS);

        let listening = SpeechRecognizeState {
            phase: ListenPhase::Listening,
            elapsed_ms: 4_200,
            ..Default::default()
        };
        assert_eq!(status_text(&listening), "聞き取り中 4秒");

        let done = SpeechRecognizeState {
            status: "認識結果: こんにちは".to_string(),
            ..Default::default()
        };
        assert_eq!(status_text(&done), done.status);
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = SpeechRecognizeState {
            phase: ListenPhase::Stopping,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "話す", &mut state));
        assert!(!click_button(&screen, "停止", &mut state));
    }

    #[test]
    fn start_button_is_clickable_when_idle() {
        use crate::click_button;
        let mut state = SpeechRecognizeState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "話す", &mut state));
        assert_eq!(state.phase, ListenPhase::Requested);
    }

    #[test]
    fn stop_button_is_clickable_while_listening() {
        use crate::click_button;
        let mut state = SpeechRecognizeState {
            phase: ListenPhase::Listening,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(click_button(&screen, "停止", &mut state));
        assert_eq!(state.phase, ListenPhase::StopRequested);
    }
}
