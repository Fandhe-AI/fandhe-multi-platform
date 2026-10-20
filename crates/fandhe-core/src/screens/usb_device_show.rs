//! Phase 11（端末機能の拡充ループ、親 #112）型サンプル「USB 機器」（Issue #505）。
//! 画面を開くと接続中の USB 機器（ベンダー ID・製品 ID・製品名）を OS から 1 回読んで一覧にし、
//! 「再取得」で読み直す 1 画面。`FANDHE_PROXY_APP=usb_device_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod usb_device_show;` と対）。機器は開かず、読み書きも扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/usb_device.rs` が [`take_request`] で要求を観測し
//! [`finish_read`] で結果を注入する（`audio_devices_show.rs` と同型の状態機械）。
//!
//! 全対象 OS の状態・文言をここで確定済みで、後続の OS 実装（#506 / #507 / #508）は本画面を
//! 触らない。画面モードは [`ListMode`]:
//!
//! - [`ListMode::System`]（既定）: 起動時に 1 回読み、`Idle` の間「再取得」を出す
//! - [`ListMode::BrowserPicker`]（Web 専用。#508 が [`UsbDeviceShowState::browser_picker`] で
//!   マウントする）: WebUSB / WebHID は利用者操作（user activation）が要るため起動時に要求を立てず、
//!   「USB 機器を選ぶ」「HID 機器を選ぶ」の 2 ボタンを常に出す（Web の Screen は初期状態から 1 回
//!   だけ組み立てられ静的なため）。選択ダイアログを閉じた場合は [`ReadFailure::Cancelled`] で、
//!   失敗にせず status 側へ出す（契約表 10）
//!
//! 画面から OS を cfg で判定しない（共通コアに target cfg を持ち込まない）。
//!
//! # 0 件と失敗の区別
//!
//! 0 件は成功として [`EMPTY_STATUS`] を出し、失敗とは別物（契約表 6）。失敗・キャンセルでは前回の
//! 一覧を残さない。
//!
//! ```text
//! Idle --「再取得」/ 選ぶ--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_reading（任意）--> Reading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const PICK_USB_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const PICK_HID_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 画面タイトル。
pub const TITLE: &str = "USB 機器";
/// 再取得ボタンのラベル。
pub const READ_LABEL: &str = "再取得";
/// Web: USB 機器の選択ボタン。
pub const PICK_USB_LABEL: &str = "USB 機器を選ぶ";
/// Web: HID 機器の選択ボタン。
pub const PICK_HID_LABEL: &str = "HID 機器を選ぶ";
/// 読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// 読み取り完了の案内。
pub const DONE_STATUS: &str = "読み取りました";
/// 0 件の案内（成功）。
pub const EMPTY_STATUS: &str = "接続中の USB 機器はありません";
/// 名前を取れなかった項目の表示。
pub const NAMELESS_TEXT: &str = "（名前を取得できません）";
/// 選択ダイアログを閉じたときの案内（失敗として見せない）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境では USB 機器の一覧を取得できません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 画面モード。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListMode {
    /// OS が全件を返す（デスクトップ・Android）。
    #[default]
    System,
    /// 利用者がブラウザの選択ダイアログで選ぶ（Web）。
    BrowserPicker,
}

/// 読み方の種別。要求に載せて effect ドライバへ渡す。ネイティブは `System` だけを処理する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadSource {
    /// OS の列挙。
    System,
    /// WebUSB（`navigator.usb.requestDevice`）。
    WebUsb,
    /// WebHID（`navigator.hid.requestDevice`）。
    WebHid,
}

/// 表示用の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDeviceRow {
    /// ベンダー ID。
    pub vendor_id: u16,
    /// 製品 ID。
    pub product_id: u16,
    /// 製品名。`None` は取れなかった項目。
    pub name: Option<String>,
}

/// 読み取り操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// 要求が立ち、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが読んでいる最中（[`mark_reading`] 済み）。
    Reading,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// 非対応。
    Unsupported,
    /// 利用者が選択ダイアログを閉じた（Web）。失敗として見せない。
    Cancelled,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDeviceShowState {
    /// 画面モード。
    pub mode: ListMode,
    /// 操作の進行状態。
    pub phase: ReadPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 直近の要求の読み方。
    pub source: ReadSource,
    /// 読み取れた一覧。失敗・キャンセル・読み取り中は `None`（前回値を残さない）。
    pub rows: Option<Vec<UsbDeviceRow>>,
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功・キャンセル時は空。
    pub error: String,
}

impl Default for UsbDeviceShowState {
    /// 起動時に 1 回読む。最初のフレーム末尾の effect が要求を観測する。
    fn default() -> Self {
        Self {
            mode: ListMode::System,
            phase: ReadPhase::Requested,
            generation: 1,
            source: ReadSource::System,
            rows: None,
            status: String::new(),
            error: String::new(),
        }
    }
}

impl UsbDeviceShowState {
    /// Web 用。起動時に要求を立てず、2 つの選択ボタンから始める（`requestDevice` は利用者操作が要る）。
    pub fn browser_picker() -> Self {
        Self {
            mode: ListMode::BrowserPicker,
            phase: ReadPhase::Idle,
            generation: 0,
            ..Self::default()
        }
    }
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRequest {
    /// [`finish_read`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 読み方。
    pub source: ReadSource,
}

fn request(state: &mut UsbDeviceShowState, source: ReadSource) {
    // `Idle` 以外では二重要求を防ぐため no-op。
    if state.phase != ReadPhase::Idle {
        return;
    }
    state.phase = ReadPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.source = source;
    // 前回の結果を残さない。
    state.rows = None;
    state.status = String::new();
    state.error = String::new();
}

/// 「再取得」ボタンのハンドラ。
pub fn read(state: &mut UsbDeviceShowState) {
    request(state, ReadSource::System);
}

/// 「USB 機器を選ぶ」ボタンのハンドラ（Web）。
pub fn pick_usb(state: &mut UsbDeviceShowState) {
    request(state, ReadSource::WebUsb);
}

/// 「HID 機器を選ぶ」ボタンのハンドラ（Web）。
pub fn pick_hid(state: &mut UsbDeviceShowState) {
    request(state, ReadSource::WebHid);
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &UsbDeviceShowState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
        source: state.source,
    })
}

/// 読み取り中（`Reading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_reading(state: &mut UsbDeviceShowState) {
    if state.phase == ReadPhase::Requested {
        state.phase = ReadPhase::Reading;
        state.status = READING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut UsbDeviceShowState,
    generation: u64,
    result: Result<Vec<UsbDeviceRow>, ReadFailure>,
) {
    if state.phase == ReadPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReadPhase::Idle;
    state.rows = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(rows) => {
            state.status = if rows.is_empty() {
                EMPTY_STATUS
            } else {
                DONE_STATUS
            }
            .to_string();
            state.rows = Some(rows);
        }
        Err(ReadFailure::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Err(ReadFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ReadFailure::Failed(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 一覧の表示（件数行 + 1 行 1 件）。未読み取りのときは空。
pub fn list_text(state: &UsbDeviceShowState) -> String {
    let Some(rows) = &state.rows else {
        return String::new();
    };
    let mut text = format!("{} 件", rows.len());
    for r in rows {
        text.push_str(&format!(
            "\nVID 0x{:04x} / PID 0x{:04x}  {}",
            r.vendor_id,
            r.product_id,
            r.name.as_deref().unwrap_or(NAMELESS_TEXT)
        ));
    }
    text
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &UsbDeviceShowState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &UsbDeviceShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`System` は `Idle` のときのみ再取得ボタン、`BrowserPicker` は 2 ボタンを常に出す
/// （各ハンドラが `Idle` 以外では no-op）。
pub fn current_screen(state: &UsbDeviceShowState) -> Screen<UsbDeviceShowState> {
    let mut screen = Screen::new(TITLE)
        .dynamic_text(status_text)
        .error_text(error_text);
    // 操作を前、長い一覧を最後に置く（窓外へ出てもボタンへ届く。Issue #77）。
    match state.mode {
        ListMode::System if state.phase == ReadPhase::Idle => {
            screen = screen.button(READ_BUTTON_ID, READ_LABEL, read);
        }
        ListMode::System => {}
        ListMode::BrowserPicker => {
            screen = screen
                .button(PICK_USB_BUTTON_ID, PICK_USB_LABEL, pick_usb)
                .button(PICK_HID_BUTTON_ID, PICK_HID_LABEL, pick_hid);
        }
    }
    screen.dynamic_text(list_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn rows() -> Vec<UsbDeviceRow> {
        vec![
            UsbDeviceRow {
                vendor_id: 0x05ac,
                product_id: 0x024f,
                name: Some("Keyboard".into()),
            },
            UsbDeviceRow {
                vendor_id: 1,
                product_id: 2,
                name: None,
            },
        ]
    }

    #[test]
    fn default_requests_a_system_read_at_startup() {
        let state = UsbDeviceShowState::default();
        assert_eq!(
            take_request(&state),
            Some(ReadRequest {
                generation: 1,
                source: ReadSource::System
            })
        );
    }

    #[test]
    fn browser_picker_has_no_startup_request_and_buttons_set_source() {
        let mut state = UsbDeviceShowState::browser_picker();
        assert_eq!(take_request(&state), None);
        let screen = current_screen(&state);
        assert!(click_button(&screen, PICK_USB_LABEL, &mut state));
        assert_eq!(take_request(&state).unwrap().source, ReadSource::WebUsb);
        let mut state = UsbDeviceShowState::browser_picker();
        let screen = current_screen(&state);
        assert!(click_button(&screen, PICK_HID_LABEL, &mut state));
        assert_eq!(take_request(&state).unwrap().source, ReadSource::WebHid);
    }

    #[test]
    fn requests_are_noop_while_in_flight() {
        let mut state = UsbDeviceShowState::default();
        mark_reading(&mut state);
        read(&mut state);
        pick_usb(&mut state);
        assert_eq!(state.phase, ReadPhase::Reading);
        assert_eq!(state.generation, 1);
        assert_eq!(state.source, ReadSource::System);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = UsbDeviceShowState::default();
        finish_read(&mut state, 99, Ok(rows()));
        assert_eq!(state.phase, ReadPhase::Requested);
        assert_eq!(state.rows, None);
        let mut idle = UsbDeviceShowState {
            phase: ReadPhase::Idle,
            ..Default::default()
        };
        finish_read(&mut idle, 1, Ok(rows()));
        assert_eq!(idle.rows, None);
    }

    #[test]
    fn cancelled_is_status_not_error_and_clears_list() {
        let mut state = UsbDeviceShowState::browser_picker();
        pick_usb(&mut state);
        finish_read(&mut state, 1, Ok(rows()));
        pick_hid(&mut state);
        finish_read(&mut state, 2, Err(ReadFailure::Cancelled));
        assert_eq!(state.phase, ReadPhase::Idle);
        assert_eq!(state.error, "");
        assert_eq!(state.status, CANCELLED_STATUS);
        assert_eq!(list_text(&state), "");
    }

    #[test]
    fn unsupported_and_failed_show_error_and_clear_list() {
        let mut state = UsbDeviceShowState::default();
        finish_read(&mut state, 1, Ok(rows()));
        read(&mut state);
        finish_read(&mut state, 2, Err(ReadFailure::Unsupported));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!((state.status.as_str(), &state.rows), ("", &None));
        read(&mut state);
        finish_read(&mut state, 3, Err(ReadFailure::Failed("x".into())));
        assert_eq!(state.error, format!("{FAILED_PREFIX}x"));
        assert_eq!(state.rows, None);
    }

    #[test]
    fn zero_rows_is_success_distinct_from_failure() {
        let mut state = UsbDeviceShowState::default();
        finish_read(&mut state, 1, Ok(vec![]));
        assert_eq!(state.status, EMPTY_STATUS);
        assert_eq!(state.error, "");
        assert_eq!(list_text(&state), "0 件");
    }

    #[test]
    fn list_shows_ids_names_and_nameless() {
        let mut state = UsbDeviceShowState::default();
        finish_read(&mut state, 1, Ok(rows()));
        assert_eq!(
            list_text(&state),
            format!(
                "2 件\nVID 0x05ac / PID 0x024f  Keyboard\nVID 0x0001 / PID 0x0002  {NAMELESS_TEXT}"
            )
        );
    }

    #[test]
    fn read_button_is_hidden_while_in_flight() {
        let mut state = UsbDeviceShowState {
            phase: ReadPhase::Idle,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(click_button(&screen, READ_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, READ_LABEL, &mut state));
    }
}
