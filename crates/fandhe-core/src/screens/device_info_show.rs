//! Phase 11（端末機能の拡充ループ、親 #1131）型サンプル「端末・OS 情報」（Issue #1304）。
//! 「端末情報を読む」ボタン → OS から OS 名・版・機種・メモリ量・空き容量を 1 回読み、5 行で
//! 表示する 1 画面。`FANDHE_PROXY_APP=device_info_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod device_info_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/device_info.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を注入する
//! （`pedometer_read.rs` と同型の状態機械）。
//!
//! 全対象 OS（macOS / Windows / iOS / Android / Web）の文言・状態をここで確定済みで、後続の
//! OS 実装（#1305 / #1306 / #1307）は本画面を触らない。Web は空き容量を取れないため `None`
//! （「不明」）で渡す（`StorageManager.estimate()` は端末の空き容量ではない）。
//!
//! # 不明と 0 の区別
//!
//! 取れなかった項目は `None` で「不明」と表示する。空き容量 `Some(0)` は「0.0 GiB」で、不明とは
//! 別物（`device-bridge-contracts.md`「6. 嘘の成功」）。非対応・失敗では行を残さない。
//!
//! ```text
//! Idle --「端末情報を読む」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_reading（同期 OS は省略可）--> Reading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 読み取りボタンの `WidgetId`。
const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 読み取りボタンのラベル。
pub const READ_LABEL: &str = "端末情報を読む";
/// 読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// 読み取り完了の案内。
pub const DONE_STATUS: &str = "読み取りました";
/// 取れなかった項目の表示。
pub const UNKNOWN_TEXT: &str = "不明";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境では端末情報を取得できません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 行ラベル: OS 名。
pub const OS_LABEL: &str = "OS";
/// 行ラベル: OS の版。
pub const OS_VERSION_LABEL: &str = "OS の版";
/// 行ラベル: 機種。
pub const MODEL_LABEL: &str = "機種";
/// 行ラベル: メモリ。
pub const MEMORY_LABEL: &str = "メモリ";
/// 行ラベル: 空き容量。OS の見積りであることを明示する。
pub const FREE_STORAGE_LABEL: &str = "空き容量（OS の見積り）";

/// 読み取った端末情報（`None` は取れなかった項目）。`fandhe_device` 側の値の写し。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceInfoReading {
    /// OS 名。
    pub os_name: Option<String>,
    /// OS の版。
    pub os_version: Option<String>,
    /// 機種名。
    pub model: Option<String>,
    /// 物理メモリ量（バイト）。
    pub memory_bytes: Option<u64>,
    /// 空き容量（バイト）。0 は有効値。
    pub free_storage_bytes: Option<u64>,
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
pub struct DeviceInfoShowState {
    /// 操作の進行状態。
    pub phase: ReadPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読み取れた端末情報。失敗・読み取り中は `None`（前回値を残さない）。
    pub reading: Option<DeviceInfoReading>,
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

/// 「端末情報を読む」ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn read(state: &mut DeviceInfoShowState) {
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
pub fn take_request(state: &DeviceInfoShowState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 読み取り中（`Reading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_reading(state: &mut DeviceInfoShowState) {
    if state.phase == ReadPhase::Requested {
        state.phase = ReadPhase::Reading;
        state.status = READING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut DeviceInfoShowState,
    generation: u64,
    result: Result<DeviceInfoReading, ReadFailure>,
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

/// バイト数を `x.y GiB`（1024³、小数 1 桁）で表す。
pub fn format_gib(bytes: u64) -> String {
    // 10 倍値で丸めて整数演算にし、浮動小数の誤差を避ける。
    let tenths = (u128::from(bytes) * 10 + (1 << 29)) >> 30;
    format!("{}.{} GiB", tenths / 10, tenths % 10)
}

fn row(
    state: &DeviceInfoShowState,
    label: &str,
    pick: fn(&DeviceInfoReading) -> Option<String>,
) -> String {
    match &state.reading {
        None => String::new(),
        Some(r) => format!(
            "{label}: {}",
            pick(r).unwrap_or_else(|| UNKNOWN_TEXT.to_string())
        ),
    }
}

/// OS 名の行。未読み取りのときは空。
pub fn os_text(state: &DeviceInfoShowState) -> String {
    row(state, OS_LABEL, |r| r.os_name.clone())
}

/// OS の版の行。
pub fn os_version_text(state: &DeviceInfoShowState) -> String {
    row(state, OS_VERSION_LABEL, |r| r.os_version.clone())
}

/// 機種の行。
pub fn model_text(state: &DeviceInfoShowState) -> String {
    row(state, MODEL_LABEL, |r| r.model.clone())
}

/// メモリの行。
pub fn memory_text(state: &DeviceInfoShowState) -> String {
    row(state, MEMORY_LABEL, |r| r.memory_bytes.map(format_gib))
}

/// 空き容量の行。
pub fn free_storage_text(state: &DeviceInfoShowState) -> String {
    row(state, FREE_STORAGE_LABEL, |r| {
        r.free_storage_bytes.map(format_gib)
    })
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &DeviceInfoShowState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &DeviceInfoShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &DeviceInfoShowState) -> Screen<DeviceInfoShowState> {
    let screen = Screen::new("端末・OS 情報")
        .dynamic_text(status_text)
        .dynamic_text(os_text)
        .dynamic_text(os_version_text)
        .dynamic_text(model_text)
        .dynamic_text(memory_text)
        .dynamic_text(free_storage_text)
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

    fn requested() -> DeviceInfoShowState {
        let mut state = DeviceInfoShowState::default();
        read(&mut state);
        state
    }

    fn info() -> DeviceInfoReading {
        DeviceInfoReading {
            os_name: Some("macOS".into()),
            os_version: Some("14.5.0".into()),
            model: None,
            memory_bytes: Some(16 << 30),
            free_storage_bytes: Some(0),
        }
    }

    #[test]
    fn read_advances_generation_and_phase() {
        let mut state = DeviceInfoShowState::default();
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
        finish_read(&mut state, 99, Ok(info()));
        assert_eq!(state.phase, ReadPhase::Requested);
        assert_eq!(state.reading, None);
        let mut idle = DeviceInfoShowState::default();
        finish_read(&mut idle, 0, Ok(info()));
        assert_eq!(idle.reading, None);
    }

    #[test]
    fn failure_clears_previous_reading() {
        let mut state = requested();
        finish_read(&mut state, 1, Ok(info()));
        read(&mut state);
        assert_eq!(state.reading, None);
        finish_read(&mut state, 2, Err(ReadFailure::Failed("x".into())));
        assert_eq!(state.reading, None);
        assert_eq!(state.status, "");
        assert_eq!(state.error, format!("{FAILED_PREFIX}x"));
        assert_eq!(os_text(&state), "");
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
    fn rows_show_unknown_and_zero_distinctly() {
        let mut state = requested();
        finish_read(&mut state, 1, Ok(info()));
        assert_eq!(state.status, DONE_STATUS);
        assert_eq!(os_text(&state), "OS: macOS");
        assert_eq!(model_text(&state), format!("機種: {UNKNOWN_TEXT}"));
        assert_eq!(memory_text(&state), "メモリ: 16.0 GiB");
        assert_eq!(
            free_storage_text(&state),
            format!("{FREE_STORAGE_LABEL}: 0.0 GiB")
        );
        assert!(FREE_STORAGE_LABEL.contains("見積り"));
    }

    #[test]
    fn format_gib_boundaries() {
        assert_eq!(format_gib(0), "0.0 GiB");
        assert_eq!(format_gib(1 << 30), "1.0 GiB");
        assert_eq!(format_gib((3 << 29) + 1), "1.5 GiB");
        assert_eq!(format_gib(u64::MAX), "17179869184.0 GiB");
    }

    #[test]
    fn button_is_hidden_while_in_flight() {
        let mut state = DeviceInfoShowState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, READ_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, READ_LABEL, &mut state));
    }
}
