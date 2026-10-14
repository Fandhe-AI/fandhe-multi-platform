//! Phase 11（端末機能の拡充ループ、親 #1119）型サンプル「歩数」（Issue #1247）。
//! 「歩数を取得」ボタン → OS から累積歩数を 1 回取得し、結果（歩数/非対応/権限拒否/未取得/失敗）を
//! 表示する 1 画面。`FANDHE_PROXY_APP=pedometer_read` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod pedometer_read;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/pedometer.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を注入する
//! （`haptics_tap.rs` と同型の状態機械）。
//!
//! 全対象 OS（Windows / iOS / Android）の文言・状態をここで定義済みで、後続の OS 実装は
//! 本画面を触らない。権限要求は取得ボタン押下時に OS 側が出すため、登録・解除ボタンは無い。
//!
//! # 0 歩と未取得の区別
//!
//! `Ok(0)` は「0 歩」と表示する正当な成功。非対応・権限拒否・未取得・失敗は `steps = None` にし、
//! 成功文言を残さない（`device-bridge-contracts.md`「6. 嘘の成功」）。
//!
//! ```text
//! Idle --「歩数を取得」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_reading--> Reading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 取得ボタンの `WidgetId`。
const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 取得ボタンのラベル。
pub const READ_LABEL: &str = "歩数を取得";
/// 取得中の案内。
pub const READING_STATUS: &str = "取得中…";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この端末では歩数を取得できません";
/// 権限拒否のときの案内。
pub const PERMISSION_DENIED_STATUS: &str = "歩数の利用が許可されていません（端末の設定で「モーションとフィットネス」/「身体活動」を許可してください）";
/// 期限内に値が届かなかったときの案内。0 歩とは別物として扱う。
pub const NO_READING_STATUS: &str = "歩数を取得できませんでした（値が届きませんでした）";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 取得操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS から取得している最中（[`mark_reading`] 済み）。
    Reading,
}

/// 失敗の種別（`fandhe_device::pedometer::PedometerFailure` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// 非対応。
    Unsupported,
    /// 権限拒否。
    PermissionDenied,
    /// 期限内に値が届かない・OS が値を持たない。
    NoReading,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 歩数画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PedometerReadState {
    /// 操作の進行状態。
    pub phase: ReadPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 取得できた歩数。失敗・取得中は `None`（0 歩は `Some(0)`）。
    pub steps: Option<u64>,
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空。
    pub error: String,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRequest {
    /// [`finish_read`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 「歩数を取得」ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn read(state: &mut PedometerReadState) {
    if state.phase != ReadPhase::Idle {
        return;
    }
    state.phase = ReadPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果を残さない。
    state.steps = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &PedometerReadState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 取得中（`Reading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_reading(state: &mut PedometerReadState) {
    if state.phase == ReadPhase::Requested {
        state.phase = ReadPhase::Reading;
        state.status = READING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut PedometerReadState,
    generation: u64,
    result: Result<u64, ReadFailure>,
) {
    if state.phase == ReadPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReadPhase::Idle;
    state.steps = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(n) => {
            state.steps = Some(n);
            state.status = format!("{n} 歩");
        }
        Err(ReadFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ReadFailure::PermissionDenied) => state.error = PERMISSION_DENIED_STATUS.to_string(),
        Err(ReadFailure::NoReading) => state.error = NO_READING_STATUS.to_string(),
        Err(ReadFailure::Failed(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &PedometerReadState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &PedometerReadState) -> String {
    state.error.clone()
}

/// 歩数画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &PedometerReadState) -> Screen<PedometerReadState> {
    let screen = Screen::new("歩数")
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != ReadPhase::Idle {
        return screen;
    }
    screen.button(READ_BUTTON_ID, READ_LABEL, read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn reading() -> PedometerReadState {
        let mut state = PedometerReadState::default();
        read(&mut state);
        mark_reading(&mut state);
        state
    }

    #[test]
    fn read_advances_generation_and_phase() {
        let mut state = PedometerReadState::default();
        read(&mut state);
        assert_eq!(state.phase, ReadPhase::Requested);
        assert_eq!(state.generation, 1);
        assert_eq!(take_request(&state), Some(ReadRequest { generation: 1 }));
        mark_reading(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn read_is_noop_while_in_flight() {
        let mut state = reading();
        read(&mut state);
        assert_eq!(state.phase, ReadPhase::Reading);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn zero_steps_is_success_distinct_from_failure() {
        let mut state = reading();
        finish_read(&mut state, 1, Ok(0));
        assert_eq!(state.steps, Some(0));
        assert_eq!(state.status, "0 歩");
        assert_eq!(state.error, "");
    }

    #[test]
    fn every_failure_clears_steps_and_success_text() {
        let failures = [
            (ReadFailure::Unsupported, UNSUPPORTED_STATUS.to_string()),
            (
                ReadFailure::PermissionDenied,
                PERMISSION_DENIED_STATUS.to_string(),
            ),
            (ReadFailure::NoReading, NO_READING_STATUS.to_string()),
            (ReadFailure::Failed("x".into()), format!("{FAILED_PREFIX}x")),
        ];
        for (f, expected) in failures {
            let mut state = reading();
            state.steps = Some(5);
            finish_read(&mut state, 1, Err(f));
            assert_eq!(state.phase, ReadPhase::Idle);
            assert_eq!(state.steps, None);
            assert_eq!(state.status, "");
            assert_eq!(state.error, expected);
        }
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = reading();
        finish_read(&mut state, 99, Ok(7));
        assert_eq!(state.phase, ReadPhase::Reading);
        assert_eq!(state.steps, None);
        let mut idle = PedometerReadState::default();
        finish_read(&mut idle, 0, Ok(7));
        assert_eq!(idle.steps, None);
    }

    #[test]
    fn button_is_hidden_while_in_flight() {
        let mut state = PedometerReadState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, READ_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, READ_LABEL, &mut state));
    }
}
