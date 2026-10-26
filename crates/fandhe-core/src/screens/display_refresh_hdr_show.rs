//! Phase 11（端末機能の拡充ループ、親 #177）型サンプル「画面のリフレッシュレート・HDR・広色域」
//! （Issue #870）。「画面の表示能力を読む」ボタン → OS から今と最大のリフレッシュレート・HDR・
//! 広色域を 1 回読み、4 行で表示する 1 画面。`FANDHE_PROXY_APP=display_refresh_hdr_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod display_refresh_hdr_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/display_refresh_hdr.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を
//! 注入する（`device_info_show.rs` と同型の状態機械）。
//!
//! 全対象 OS の文言・状態をここで確定済みで、後続の OS 実装（#871 / #872）と Web（#873）は
//! 本画面を触らない。
//!
//! # 取れないと「いいえ」の区別
//!
//! 取れなかった項目は `None` で「この OS では取れない」と表示する。HDR の `Some(false)` は
//! 「いいえ」で、取れないとは別物（`device-bridge-contracts.md`「6. 嘘の成功」）。
//! 非対応・失敗では行を残さない。
//!
//! ```text
//! Idle --「画面の表示能力を読む」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_reading（同期 OS は省略可）--> Reading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 読み取りボタンの `WidgetId`。
const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 画面名。
pub const SCREEN_TITLE: &str = "画面のリフレッシュレート・HDR・広色域";
/// 読み取りボタンのラベル。
pub const READ_LABEL: &str = "画面の表示能力を読む";
/// 読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// 読み取り完了の案内。
pub const DONE_STATUS: &str = "読み取りました";
/// 取れなかった項目の表示。
pub const UNKNOWN_TEXT: &str = "この OS では取れない";
/// 真偽の「はい」。
pub const YES_TEXT: &str = "はい";
/// 真偽の「いいえ」。
pub const NO_TEXT: &str = "いいえ";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境では画面の表示能力を取得できません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 行ラベル: 今のリフレッシュレート。
pub const CURRENT_HZ_LABEL: &str = "今のリフレッシュレート";
/// 行ラベル: 最大のリフレッシュレート。
pub const MAX_HZ_LABEL: &str = "最大のリフレッシュレート";
/// 行ラベル: HDR。
pub const HDR_LABEL: &str = "HDR";
/// 行ラベル: 広色域。
pub const WIDE_GAMUT_LABEL: &str = "広色域（P3）";

/// 読み取った画面の表示能力（`None` は取れなかった項目）。`fandhe_device` 側の値の写し。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayReading {
    /// 今のリフレッシュレート（Hz）。
    pub current_hz: Option<u32>,
    /// 最大のリフレッシュレート（Hz）。
    pub max_hz: Option<u32>,
    /// HDR 対応。`Some(false)` は「いいえ」。
    pub hdr: Option<bool>,
    /// 広色域（P3）。
    pub wide_gamut: Option<bool>,
}

/// 読み取り操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS から読んでいる最中（[`mark_reading`] 済み。非同期 API の OS 用）。
    Reading,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// 非対応。
    Unsupported,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayRefreshHdrShowState {
    /// 操作の進行状態。
    pub phase: ReadPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読み取れた表示能力。失敗・読み取り中は `None`（前回値を残さない）。
    pub reading: Option<DisplayReading>,
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

/// 「画面の表示能力を読む」ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn read(state: &mut DisplayRefreshHdrShowState) {
    if state.phase != ReadPhase::Idle {
        return;
    }
    state.phase = ReadPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果を残さない。
    state.reading = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &DisplayRefreshHdrShowState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 読み取り中（`Reading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_reading(state: &mut DisplayRefreshHdrShowState) {
    if state.phase == ReadPhase::Requested {
        state.phase = ReadPhase::Reading;
        state.status = READING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut DisplayRefreshHdrShowState,
    generation: u64,
    result: Result<DisplayReading, ReadFailure>,
) {
    if state.phase == ReadPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReadPhase::Idle;
    state.reading = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(r) => {
            state.reading = Some(r);
            state.status = DONE_STATUS.to_string();
        }
        Err(ReadFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ReadFailure::Failed(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

fn hz_text(hz: Option<u32>) -> String {
    hz.map_or_else(|| UNKNOWN_TEXT.to_string(), |n| format!("{n} Hz"))
}

fn bool_text(b: Option<bool>) -> String {
    match b {
        Some(true) => YES_TEXT.to_string(),
        Some(false) => NO_TEXT.to_string(),
        None => UNKNOWN_TEXT.to_string(),
    }
}

fn row(
    state: &DisplayRefreshHdrShowState,
    label: &str,
    pick: fn(&DisplayReading) -> String,
) -> String {
    match &state.reading {
        None => String::new(),
        Some(r) => format!("{label}: {}", pick(r)),
    }
}

/// 今のリフレッシュレートの行。未読み取りのときは空。
pub fn current_hz_row(state: &DisplayRefreshHdrShowState) -> String {
    row(state, CURRENT_HZ_LABEL, |r| hz_text(r.current_hz))
}

/// 最大のリフレッシュレートの行。
pub fn max_hz_row(state: &DisplayRefreshHdrShowState) -> String {
    row(state, MAX_HZ_LABEL, |r| hz_text(r.max_hz))
}

/// HDR の行。
pub fn hdr_row(state: &DisplayRefreshHdrShowState) -> String {
    row(state, HDR_LABEL, |r| bool_text(r.hdr))
}

/// 広色域の行。
pub fn wide_gamut_row(state: &DisplayRefreshHdrShowState) -> String {
    row(state, WIDE_GAMUT_LABEL, |r| bool_text(r.wide_gamut))
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &DisplayRefreshHdrShowState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &DisplayRefreshHdrShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &DisplayRefreshHdrShowState) -> Screen<DisplayRefreshHdrShowState> {
    let screen = Screen::new(SCREEN_TITLE)
        .dynamic_text(status_text)
        .dynamic_text(current_hz_row)
        .dynamic_text(max_hz_row)
        .dynamic_text(hdr_row)
        .dynamic_text(wide_gamut_row)
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

    fn requested() -> DisplayRefreshHdrShowState {
        let mut state = DisplayRefreshHdrShowState::default();
        read(&mut state);
        state
    }

    fn sample() -> DisplayReading {
        DisplayReading {
            current_hz: Some(120),
            max_hz: None,
            hdr: Some(false),
            wide_gamut: Some(true),
        }
    }

    #[test]
    fn read_advances_generation_and_phase() {
        let mut state = DisplayRefreshHdrShowState::default();
        read(&mut state);
        assert_eq!(state.phase, ReadPhase::Requested);
        assert_eq!(state.generation, 1);
        assert_eq!(take_request(&state), Some(ReadRequest { generation: 1 }));
        mark_reading(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn read_is_noop_while_in_flight() {
        let mut state = requested();
        mark_reading(&mut state);
        read(&mut state);
        assert_eq!(state.phase, ReadPhase::Reading);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = requested();
        finish_read(&mut state, 99, Ok(sample()));
        assert_eq!(state.phase, ReadPhase::Requested);
        assert_eq!(state.reading, None);
        let mut idle = DisplayRefreshHdrShowState::default();
        finish_read(&mut idle, 0, Ok(sample()));
        assert_eq!(idle.reading, None);
    }

    #[test]
    fn press_clears_previous_reading() {
        let mut state = requested();
        finish_read(&mut state, 1, Ok(sample()));
        assert!(state.reading.is_some());
        read(&mut state);
        assert_eq!(state.reading, None);
        assert_eq!(state.status, "");
    }

    #[test]
    fn failure_shows_no_rows() {
        let mut state = requested();
        finish_read(&mut state, 1, Err(ReadFailure::Failed("x".into())));
        assert_eq!(state.reading, None);
        assert_eq!(state.status, "");
        assert_eq!(state.error, format!("{FAILED_PREFIX}x"));
        assert_eq!(hdr_row(&state), "");
    }

    #[test]
    fn unsupported_shows_no_success_text() {
        let mut state = requested();
        finish_read(&mut state, 1, Err(ReadFailure::Unsupported));
        assert_eq!(state.phase, ReadPhase::Idle);
        assert_eq!(state.status, "");
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.reading, None);
    }

    #[test]
    fn rows_distinguish_unknown_no_and_yes() {
        let mut state = requested();
        finish_read(&mut state, 1, Ok(sample()));
        assert_eq!(state.status, DONE_STATUS);
        assert_eq!(current_hz_row(&state), "今のリフレッシュレート: 120 Hz");
        assert_eq!(
            max_hz_row(&state),
            format!("最大のリフレッシュレート: {UNKNOWN_TEXT}")
        );
        assert_eq!(hdr_row(&state), "HDR: いいえ");
        assert_eq!(wide_gamut_row(&state), "広色域（P3）: はい");
    }

    #[test]
    fn button_is_hidden_while_in_flight() {
        let mut state = DisplayRefreshHdrShowState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, READ_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, READ_LABEL, &mut state));
    }
}
