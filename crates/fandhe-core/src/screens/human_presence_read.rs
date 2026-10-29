//! Phase 11（端末機能の拡充ループ、親 #187）型サンプル「人の在席・離席の検知」（Issue #960）。
//! 「いま在席しているかを読む」ボタン → OS から在席を 1 回読み、結果（在席/離席/判定不能/
//! 非対応/権限拒否/未取得/失敗）を表示する 1 画面。`FANDHE_PROXY_APP=human_presence_read` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod human_presence_read;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/human_presence.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を注入する
//! （`pedometer_read.rs` と同型の状態機械）。購読しないため登録・解除ボタンは無い。
//!
//! センサーの無い環境（macOS 等）に代替入力は置かない。値を偽装する入力は嘘の成功の入口になるため、
//! `Unsupported` の文言をそのまま出す。
//!
//! # 判定不能と失敗の区別
//!
//! `Unknown` は OS が返した成功値なので `status` に出し、`error` には出さない。ただし在席・離席の
//! どちらにも見せない。失敗は `presence = None` にし、成功文言を残さない
//! （`device-bridge-contracts.md`「6. 嘘の成功」）。
//!
//! ```text
//! Idle --「いま在席しているかを読む」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_reading--> Reading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 読み取りボタンの `WidgetId`。
const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 読み取りボタンのラベル。
pub const READ_LABEL: &str = "いま在席しているかを読む";
/// 読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// 在席。
pub const PRESENT_STATUS: &str = "在席しています（OS が人を検知しました）";
/// 離席。
pub const NOT_PRESENT_STATUS: &str = "離席しています（OS は人を検知していません）";
/// 判定不能（成功値。離席とは別物）。
pub const UNKNOWN_STATUS: &str = "OS は在席を判定できないと返しました";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str =
    "この PC では在席を検知できません（人の在席センサーがありません）";
/// 権限拒否のときの案内。
pub const PERMISSION_DENIED_STATUS: &str = "在席の検知が許可されていません（Windows の設定 → プライバシーとセキュリティ → 在席検知 で、デスクトップ アプリへのアクセスを許可してください）";
/// 期限内に値が届かなかったときの案内。離席とは別物として扱う。
pub const NO_READING_STATUS: &str = "在席を読み取れませんでした（値が届きませんでした）";
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

/// 読み取れた在席（`fandhe_device::human_presence::Presence` から写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceView {
    /// 在席。
    Present,
    /// 離席。
    NotPresent,
    /// 判定不能。
    Unknown,
}

/// 失敗の種別（`HumanPresenceFailure` から写す責務は effect 側）。
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

/// 在席画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HumanPresenceReadState {
    /// 操作の進行状態。
    pub phase: ReadPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読み取れた在席。失敗・読み取り中は `None`。
    pub presence: Option<PresenceView>,
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

/// 読み取りボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn read(state: &mut HumanPresenceReadState) {
    if state.phase != ReadPhase::Idle {
        return;
    }
    state.phase = ReadPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果を残さない。
    state.presence = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &HumanPresenceReadState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 読み取り中（`Reading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_reading(state: &mut HumanPresenceReadState) {
    if state.phase == ReadPhase::Requested {
        state.phase = ReadPhase::Reading;
        state.status = READING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut HumanPresenceReadState,
    generation: u64,
    result: Result<PresenceView, ReadFailure>,
) {
    if state.phase == ReadPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReadPhase::Idle;
    state.presence = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(v) => {
            state.presence = Some(v);
            state.status = match v {
                PresenceView::Present => PRESENT_STATUS,
                PresenceView::NotPresent => NOT_PRESENT_STATUS,
                PresenceView::Unknown => UNKNOWN_STATUS,
            }
            .to_string();
        }
        Err(ReadFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ReadFailure::PermissionDenied) => state.error = PERMISSION_DENIED_STATUS.to_string(),
        Err(ReadFailure::NoReading) => state.error = NO_READING_STATUS.to_string(),
        Err(ReadFailure::Failed(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &HumanPresenceReadState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &HumanPresenceReadState) -> String {
    state.error.clone()
}

/// 在席画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &HumanPresenceReadState) -> Screen<HumanPresenceReadState> {
    let screen = Screen::new("人の在席・離席の検知")
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

    fn reading() -> HumanPresenceReadState {
        let mut state = HumanPresenceReadState::default();
        read(&mut state);
        mark_reading(&mut state);
        state
    }

    #[test]
    fn read_advances_generation_and_phase() {
        let mut state = HumanPresenceReadState::default();
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
    fn each_presence_has_own_text_and_no_error() {
        let cases = [
            (PresenceView::Present, PRESENT_STATUS),
            (PresenceView::NotPresent, NOT_PRESENT_STATUS),
            (PresenceView::Unknown, UNKNOWN_STATUS),
        ];
        for (v, expected) in cases {
            let mut state = reading();
            finish_read(&mut state, 1, Ok(v));
            assert_eq!(state.presence, Some(v));
            assert_eq!(state.status, expected);
            assert_eq!(state.error, "");
        }
    }

    #[test]
    fn every_failure_clears_presence_and_success_text() {
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
            state.presence = Some(PresenceView::Present);
            finish_read(&mut state, 1, Err(f));
            assert_eq!(state.phase, ReadPhase::Idle);
            assert_eq!(state.presence, None);
            assert_eq!(state.status, "");
            assert_eq!(state.error, expected);
        }
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = reading();
        finish_read(&mut state, 99, Ok(PresenceView::Present));
        assert_eq!(state.phase, ReadPhase::Reading);
        assert_eq!(state.presence, None);
        let mut idle = HumanPresenceReadState::default();
        finish_read(&mut idle, 0, Ok(PresenceView::Present));
        assert_eq!(idle.presence, None);
    }

    #[test]
    fn button_is_hidden_while_in_flight() {
        let mut state = HumanPresenceReadState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, READ_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, READ_LABEL, &mut state));
    }
}
