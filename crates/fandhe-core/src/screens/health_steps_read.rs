//! Phase 11（端末機能の拡充ループ、親 #149）型サンプル「ヘルスケアの歩数」（Issue #1089）。
//! 「今日の歩数を読む」ボタン → ヘルスストアから今日の歩数を 1 回読み、結果
//! （歩数/記録が見えない/取り消し/非対応/期限切れ/失敗）を表示する 1 画面。
//! `FANDHE_PROXY_APP=health_steps_read` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod health_steps_read;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/health_data.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を注入する。
//!
//! 全対象 OS（現状 iOS のみ）の文言・状態をここで定義済みで、後続の OS 実装は本画面を触らない。
//! 許可は読み取りボタンの押下時に OS がシートで求めるため、登録・解除ボタンは無い。
//! 起動時に読みに行かない（予期しない許可シートを避ける）。
//!
//! # 0 歩と記録が見えないことの区別
//!
//! `Steps(0)` は「0 歩」と表示する正当な成功。`NoVisibleData` は専用文言で `steps = None`
//! （失敗の色にも「0 歩」にもしない）。取り消しは中立表示。非対応・期限切れ・失敗は `error` に入れる。
//!
//! ```text
//! Idle --「今日の歩数を読む」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_reading--> Reading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 読み取りボタンの `WidgetId`。
const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 読み取りボタンのラベル。
pub const READ_LABEL: &str = "今日の歩数を読む";
/// 読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…（初回は許可の確認が表示されます）";
/// 記録が見えないときの案内（失敗の色にしない。0 歩とは別物）。
pub const NO_VISIBLE_DATA_STATUS: &str = "今日の歩数の記録が見えません（記録が無いか、ヘルスケアで読み取りを許可していません。設定アプリのヘルスケアから変えられます）";
/// 取り消し（許可シートを閉じた）の案内。中立表示。
pub const CANCELED_STATUS: &str = "取り消しました";
/// 非対応のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この端末ではヘルスケアの歩数を読めません";
/// 期限内に結果が届かなかったときの案内。
pub const TIMEOUT_STATUS: &str = "応答がありませんでした";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 読み取り操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS から読んでいる最中（[`mark_reading`] 済み）。
    Reading,
}

/// 読み取りの成功値（`fandhe_device::health_data::HealthSteps` から写す責務は effect 側）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StepsReading {
    /// 今日の歩数（0 を含む）。
    Steps(u64),
    /// 記録が見えない。
    NoVisibleData,
}

/// 歩数は健康情報のため Debug 出力へ出さない（`HealthSteps` と同じ方針）。
impl std::fmt::Debug for StepsReading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Steps(_) => f.write_str("Steps(<redacted>)"),
            Self::NoVisibleData => f.write_str("NoVisibleData"),
        }
    }
}

/// 失敗の種別（`fandhe_device::health_data::HealthDataFailure` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// 非対応。
    Unsupported,
    /// 利用者の取り消し。
    Canceled,
    /// 期限切れ。
    Timeout,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 画面の状態。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct HealthStepsReadState {
    /// 操作の進行状態。
    pub phase: ReadPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読めた歩数。失敗・読み取り中・記録なしは `None`（0 歩は `Some(0)`）。
    pub steps: Option<u64>,
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功・取り消し・記録なしは空。
    pub error: String,
}

/// 歩数を含む `steps` と `status` は Debug 出力へ出さない（有無だけ示す）。
impl std::fmt::Debug for HealthStepsReadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthStepsReadState")
            .field("phase", &self.phase)
            .field("generation", &self.generation)
            .field("steps", &self.steps.map(|_| "<redacted>"))
            .field("status", &"<redacted>")
            .field("error", &self.error)
            .finish()
    }
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRequest {
    /// [`finish_read`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 読み取りボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn read(state: &mut HealthStepsReadState) {
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
pub fn take_request(state: &HealthStepsReadState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 読み取り中（`Reading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_reading(state: &mut HealthStepsReadState) {
    if state.phase == ReadPhase::Requested {
        state.phase = ReadPhase::Reading;
        state.status = READING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut HealthStepsReadState,
    generation: u64,
    result: Result<StepsReading, ReadFailure>,
) {
    if state.phase == ReadPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReadPhase::Idle;
    state.steps = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(StepsReading::Steps(n)) => {
            state.steps = Some(n);
            state.status = format!("今日の歩数: {n} 歩");
        }
        Ok(StepsReading::NoVisibleData) => state.status = NO_VISIBLE_DATA_STATUS.to_string(),
        Err(ReadFailure::Canceled) => state.status = CANCELED_STATUS.to_string(),
        Err(ReadFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ReadFailure::Timeout) => state.error = TIMEOUT_STATUS.to_string(),
        Err(ReadFailure::Failed(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &HealthStepsReadState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &HealthStepsReadState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &HealthStepsReadState) -> Screen<HealthStepsReadState> {
    let screen = Screen::new("ヘルスケアの歩数")
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

    fn reading() -> HealthStepsReadState {
        let mut state = HealthStepsReadState::default();
        read(&mut state);
        mark_reading(&mut state);
        state
    }

    #[test]
    fn debug_hides_steps() {
        let mut state = HealthStepsReadState::default();
        read(&mut state);
        finish_read(&mut state, 1, Ok(StepsReading::Steps(12345)));
        assert!(!format!("{state:?}").contains("12345"));
        assert!(!format!("{:?}", StepsReading::Steps(12345)).contains("12345"));
    }

    #[test]
    fn read_advances_generation_and_phase() {
        let mut state = HealthStepsReadState::default();
        read(&mut state);
        assert_eq!(state.phase, ReadPhase::Requested);
        assert_eq!(state.generation, 1);
        assert_eq!(take_request(&state), Some(ReadRequest { generation: 1 }));
        mark_reading(&mut state);
        assert_eq!(state.phase, ReadPhase::Reading);
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
    fn zero_steps_is_success() {
        let mut state = reading();
        finish_read(&mut state, 1, Ok(StepsReading::Steps(0)));
        assert_eq!(state.steps, Some(0));
        assert_eq!(state.status, "今日の歩数: 0 歩");
        assert_eq!(state.error, "");
    }

    #[test]
    fn no_visible_data_is_neither_zero_nor_error() {
        let mut state = reading();
        finish_read(&mut state, 1, Ok(StepsReading::NoVisibleData));
        assert_eq!(state.steps, None);
        assert_eq!(state.status, NO_VISIBLE_DATA_STATUS);
        assert_eq!(state.error, "");
        assert!(!state.status.contains("0 歩"));
    }

    #[test]
    fn canceled_is_neutral() {
        let mut state = reading();
        finish_read(&mut state, 1, Err(ReadFailure::Canceled));
        assert_eq!(state.status, CANCELED_STATUS);
        assert_eq!(state.error, "");
        assert_eq!(state.steps, None);
    }

    #[test]
    fn failures_set_error_and_clear_steps() {
        let failures = [
            (ReadFailure::Unsupported, UNSUPPORTED_STATUS.to_string()),
            (ReadFailure::Timeout, TIMEOUT_STATUS.to_string()),
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
        finish_read(&mut state, 99, Ok(StepsReading::Steps(7)));
        assert_eq!(state.phase, ReadPhase::Reading);
        assert_eq!(state.steps, None);
        let mut idle = HealthStepsReadState::default();
        finish_read(&mut idle, 0, Ok(StepsReading::Steps(7)));
        assert_eq!(idle.steps, None);
    }

    #[test]
    fn button_is_hidden_while_in_flight() {
        let mut state = HealthStepsReadState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, READ_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, READ_LABEL, &mut state));
    }
}
