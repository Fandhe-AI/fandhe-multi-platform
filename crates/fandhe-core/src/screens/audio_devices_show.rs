//! Phase 11（端末機能の拡充ループ、親 #1136）型サンプル「音声デバイス」（Issue #1330）。
//! 画面を開くと接続中の音声入力・出力デバイスの名前を OS から 1 回読んで一覧にし、「再取得」で
//! 読み直す 1 画面。`FANDHE_PROXY_APP=audio_devices_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod audio_devices_show;` と対）。範囲は一覧のみで、出力先の選択・
//! 切り替え・音量は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/audio_devices.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を注入する
//! （`device_info_show.rs` と同型の状態機械）。
//!
//! 全対象 OS（macOS / Windows / iOS / Android / Web）の文言・状態をここで確定済みで、後続の
//! OS 実装（#1331 / #1332 / #1333）は本画面を触らない。方向ごとの状態は [`DeviceGroup`]:
//! iOS の入力は [`DeviceGroup::OnlyWhileMicActive`]、Web の許可前は
//! [`DeviceGroup::AfterMicPermission`]（Web だけが使う）。OS 固有の操作 UI は無い。
//!
//! # 0 件と取得不可の区別
//!
//! `Listed(空)` は「0 件」、`Unavailable` は「取得できません」で別物
//! （`device-bridge-contracts.md`「6. 嘘の成功」）。非対応・失敗では行を残さない。
//!
//! ```text
//! 起動 = Requested（generation 1）
//! Idle --「再取得」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_reading（ワーカー OS のみ。同期 OS は省略可）--> Reading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 再取得ボタンの `WidgetId`。
const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 画面タイトル。
pub const TITLE: &str = "音声デバイス";
/// 再取得ボタンのラベル。
pub const READ_LABEL: &str = "再取得";
/// 読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// 読み取り完了の案内。
pub const DONE_STATUS: &str = "読み取りました";
/// 行ラベル: 入力。
pub const INPUT_LABEL: &str = "入力";
/// 行ラベル: 出力。
pub const OUTPUT_LABEL: &str = "出力";
/// 名前を取れなかった項目の表示。
pub const NAMELESS_TEXT: &str = "（名前を取得できません）";
/// 列挙する手段が無い方向の表示。
pub const UNAVAILABLE_TEXT: &str = "取得できません";
/// 入力を許すセッションのときだけ取れる方向の表示（iOS の入力）。
pub const ONLY_WHILE_MIC_TEXT: &str = "入力の一覧はマイク使用中のみ取得できます";
/// マイク許可の前の表示（Web だけが使う）。
pub const AFTER_MIC_PERMISSION_TEXT: &str = "名前・件数はマイクの許可後に表示されます";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境では音声デバイスの一覧を取得できません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 1 方向の表示用の状態。`fandhe_device` 側の値の写し（+ Web 専用の許可前）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceGroup {
    /// 列挙できた。名前は 1 件ずつ（`None` は名前なし）。0 件を含む。
    Listed(Vec<Option<String>>),
    /// 列挙する手段が無い。
    Unavailable,
    /// 入力を許すセッションのときだけ取れる（iOS の入力）。
    OnlyWhileMicActive,
    /// マイク許可の前（Web だけが使う）。
    AfterMicPermission,
}

/// 読み取った音声デバイス。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevicesReading {
    /// 入力デバイス。
    pub inputs: DeviceGroup,
    /// 出力デバイス。
    pub outputs: DeviceGroup,
}

/// 読み取り操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// 要求が立ち、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS から読んでいる最中（[`mark_reading`] 済み）。
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevicesShowState {
    /// 操作の進行状態。
    pub phase: ReadPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読み取れた一覧。失敗・読み取り中は `None`（前回値を残さない）。
    pub reading: Option<AudioDevicesReading>,
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空。
    pub error: String,
}

impl Default for AudioDevicesShowState {
    /// 起動時に 1 回読む。最初のフレーム末尾の effect が要求を観測する。
    fn default() -> Self {
        Self {
            phase: ReadPhase::Requested,
            generation: 1,
            reading: None,
            status: String::new(),
            error: String::new(),
        }
    }
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRequest {
    /// [`finish_read`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 「再取得」ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn read(state: &mut AudioDevicesShowState) {
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
pub fn take_request(state: &AudioDevicesShowState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 読み取り中（`Reading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_reading(state: &mut AudioDevicesShowState) {
    if state.phase == ReadPhase::Requested {
        state.phase = ReadPhase::Reading;
        state.status = READING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut AudioDevicesShowState,
    generation: u64,
    result: Result<AudioDevicesReading, ReadFailure>,
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

/// 1 方向の表示文字列。`Listed` は見出し行（件数）+ 1 行 1 件の名前。
fn group_text(label: &str, group: &DeviceGroup) -> String {
    match group {
        DeviceGroup::Listed(devices) => {
            let mut text = format!("{label}: {} 件", devices.len());
            for name in devices {
                text.push('\n');
                text.push_str(name.as_deref().unwrap_or(NAMELESS_TEXT));
            }
            text
        }
        DeviceGroup::Unavailable => format!("{label}: {UNAVAILABLE_TEXT}"),
        DeviceGroup::OnlyWhileMicActive => ONLY_WHILE_MIC_TEXT.to_string(),
        DeviceGroup::AfterMicPermission => AFTER_MIC_PERMISSION_TEXT.to_string(),
    }
}

/// 入力の表示（`DynamicText` の `get`）。未読み取りのときは空。
pub fn inputs_text(state: &AudioDevicesShowState) -> String {
    state
        .reading
        .as_ref()
        .map_or_else(String::new, |r| group_text(INPUT_LABEL, &r.inputs))
}

/// 出力の表示。未読み取りのときは空。
pub fn outputs_text(state: &AudioDevicesShowState) -> String {
    state
        .reading
        .as_ref()
        .map_or_else(String::new, |r| group_text(OUTPUT_LABEL, &r.outputs))
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &AudioDevicesShowState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &AudioDevicesShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &AudioDevicesShowState) -> Screen<AudioDevicesShowState> {
    let screen = Screen::new(TITLE)
        .dynamic_text(status_text)
        .dynamic_text(inputs_text)
        .dynamic_text(outputs_text)
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

    fn reading() -> AudioDevicesReading {
        AudioDevicesReading {
            inputs: DeviceGroup::Listed(vec![Some("Mic".into()), None]),
            outputs: DeviceGroup::Listed(vec![]),
        }
    }

    #[test]
    fn default_requests_a_read_at_startup() {
        let state = AudioDevicesShowState::default();
        assert_eq!(take_request(&state), Some(ReadRequest { generation: 1 }));
    }

    #[test]
    fn read_is_noop_while_in_flight() {
        let mut state = AudioDevicesShowState::default();
        mark_reading(&mut state);
        read(&mut state);
        assert_eq!(state.phase, ReadPhase::Reading);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = AudioDevicesShowState::default();
        finish_read(&mut state, 99, Ok(reading()));
        assert_eq!(state.phase, ReadPhase::Requested);
        assert_eq!(state.reading, None);
        let mut idle = AudioDevicesShowState {
            phase: ReadPhase::Idle,
            ..Default::default()
        };
        finish_read(&mut idle, 1, Ok(reading()));
        assert_eq!(idle.reading, None);
    }

    #[test]
    fn failure_clears_previous_reading() {
        let mut state = AudioDevicesShowState::default();
        finish_read(&mut state, 1, Ok(reading()));
        assert_eq!(state.status, DONE_STATUS);
        read(&mut state);
        assert_eq!(state.reading, None);
        finish_read(&mut state, 2, Err(ReadFailure::Failed("x".into())));
        assert_eq!(state.reading, None);
        assert_eq!(state.status, "");
        assert_eq!(state.error, format!("{FAILED_PREFIX}x"));
        assert_eq!(inputs_text(&state), "");
    }

    #[test]
    fn unsupported_shows_no_success_text() {
        let mut state = AudioDevicesShowState::default();
        finish_read(&mut state, 1, Err(ReadFailure::Unsupported));
        assert_eq!(state.phase, ReadPhase::Idle);
        assert_eq!(state.status, "");
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.reading, None);
    }

    #[test]
    fn zero_unavailable_and_other_groups_read_differently() {
        let zero = group_text(INPUT_LABEL, &DeviceGroup::Listed(vec![]));
        let none = group_text(INPUT_LABEL, &DeviceGroup::Unavailable);
        assert_eq!(zero, "入力: 0 件");
        assert_eq!(none, "入力: 取得できません");
        assert_eq!(
            group_text(INPUT_LABEL, &DeviceGroup::OnlyWhileMicActive),
            ONLY_WHILE_MIC_TEXT
        );
        assert_eq!(
            group_text(INPUT_LABEL, &DeviceGroup::AfterMicPermission),
            AFTER_MIC_PERMISSION_TEXT
        );
    }

    #[test]
    fn listed_shows_count_names_and_nameless() {
        let mut state = AudioDevicesShowState::default();
        finish_read(&mut state, 1, Ok(reading()));
        assert_eq!(
            inputs_text(&state),
            format!("入力: 2 件\nMic\n{NAMELESS_TEXT}")
        );
        assert_eq!(outputs_text(&state), "出力: 0 件");
    }

    #[test]
    fn button_is_hidden_while_in_flight() {
        let mut state = AudioDevicesShowState {
            phase: ReadPhase::Idle,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(click_button(&screen, READ_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, READ_LABEL, &mut state));
    }
}
