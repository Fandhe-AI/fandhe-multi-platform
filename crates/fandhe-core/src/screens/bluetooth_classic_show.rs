//! Phase 11 の拡充ループ型サンプル「Bluetooth Classic」（親 #130、Issue #659）。
//! 画面を開くとペアリング済みの Bluetooth Classic 機器を OS から 1 回読んで一覧にし、
//! 「検索（12 秒）」で周辺の機器を固定窓で 1 回検索して同じ一覧へ足す 1 画面。
//! `FANDHE_PROXY_APP=bluetooth_classic_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod bluetooth_classic_show;` と対）。接続・ペアリング・機器のアドレス表示は
//! 扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/bluetooth_classic.rs` が [`take_paired_request`] /
//! [`take_discover_request`] で要求を観測し、[`finish_paired`] / [`finish_discover`] で結果を注入する。
//! 状態・定数・文言は全 OS 分をここで確定し、後続の OS 実装は画面に触らない。
//!
//! # 0 件と失敗を混同しない理由
//!
//! 0 件は正常な結果で、オフ・権限拒否・非対応を「0 件」と見せると利用者は原因を取り違える
//! （`device-bridge-contracts.md`「6. 嘘の成功」）。0 件は `status`（通常色）、失敗は `error`
//! （`TextRole::Error`）へ別々に書き、失敗では行を空にする。利用者が取り消す操作は無いため
//! `Cancelled` は持たない（同 10）。
//!
//! # 一覧と検索は独立
//!
//! 一覧読みと検索は世代を別々に持ち、互いの完了を打ち消さない。
//!
//! ```text
//! 一覧: Requested(初期) --take/mark_paired_reading--> Reading --finish_paired--> Idle
//! 検索: Idle --「検索」--> Requested, generation += 1
//!       --take/mark_discovering--> Discovering --finish_discover（世代一致のみ）--> Idle
//! ```

use crate::{Node, Screen, WidgetId};

const DISCOVER_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const RELOAD_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 画面タイトル。
pub const TITLE: &str = "Bluetooth Classic";
/// 検索ボタンのラベル。
pub const DISCOVER_BUTTON_LABEL: &str = "検索（12 秒）";
/// 検索中のボタンのラベル（押しても [`start_discover`] が no-op の無効表示）。
pub const DISCOVERING_BUTTON_LABEL: &str = "検索中…";
/// 一覧の読み直しボタンのラベル。
pub const RELOAD_BUTTON_LABEL: &str = "一覧を読み直す";
/// 一覧の読み込み中の案内。
pub const READING_STATUS: &str = "読み込み中…";
/// 検索中の案内。
pub const DISCOVERING_STATUS: &str = "検索中…（約 12 秒）";
/// ペアリング済みが 0 件のときの案内（成功）。
pub const PAIRED_EMPTY_STATUS: &str = "ペアリング済みの機器はありません";
/// 検索で 0 件のときの案内（成功）。
pub const DISCOVER_EMPTY_STATUS: &str = "周辺に Bluetooth Classic の機器は見つかりませんでした";
/// Bluetooth がオフのときの失敗文言。
pub const DISABLED_ERROR: &str = "Bluetooth がオフです";
/// 権限がないときの失敗文言。
pub const PERMISSION_DENIED_ERROR: &str = "Bluetooth の使用が許可されていません";
/// 非対応のときの失敗文言。
pub const UNSUPPORTED_ERROR: &str = "この端末では Bluetooth Classic を使えません";
/// その他の失敗の接頭辞（`{接頭辞}{理由}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 名前を取れなかった機器の表記。
pub const NAMELESS_TEXT: &str = "（名前なし）";
/// ペアリング済みの機器に付ける表記。
pub const PAIRED_LABEL: &str = "ペアリング済み";
/// 検索で見つかった行に付ける接頭辞（一覧の行と区別する）。
pub const FOUND_PREFIX: &str = "検索: ";
/// 画面の注記（範囲）。
pub const NOTE: &str = "一覧と検索のみ。接続・ペアリングはしない、アドレスは出さない";

/// 機器の種別（`fandhe_device::bluetooth_classic::DeviceKind` に対応する画面用の型。
/// ラベル文言は core に置く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicKind {
    /// パソコン。
    Computer,
    /// 電話。
    Phone,
    /// ネットワーク機器。
    Network,
    /// オーディオ・ビデオ。
    AudioVideo,
    /// 入力機器など。
    Peripheral,
    /// 画像機器。
    Imaging,
    /// ウェアラブル。
    Wearable,
    /// 玩具。
    Toy,
    /// 健康機器。
    Health,
    /// その他。
    Misc,
    /// 未分類。
    Uncategorized,
    /// 不明。
    Unknown,
}

impl ClassicKind {
    /// 画面に出す種別ラベル。
    pub fn label(self) -> &'static str {
        match self {
            Self::Computer => "パソコン",
            Self::Phone => "電話",
            Self::Network => "ネットワーク",
            Self::AudioVideo => "オーディオ・ビデオ",
            Self::Peripheral => "入力機器",
            Self::Imaging => "画像機器",
            Self::Wearable => "ウェアラブル",
            Self::Toy => "玩具",
            Self::Health => "健康機器",
            Self::Misc => "その他",
            Self::Uncategorized => "未分類",
            Self::Unknown => "種別不明",
        }
    }
}

/// 一覧の 1 行（アドレスは持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassicRow {
    /// 機器名。取れなければ `None`。
    pub name: Option<String>,
    /// 種別。
    pub kind: ClassicKind,
    /// ペアリング済みか。
    pub paired: bool,
    /// 電波強度（dBm）。取れなければ `None`。
    pub rssi: Option<i8>,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassicError {
    /// 権限なし。
    PermissionDenied,
    /// Bluetooth がオフ。
    Disabled,
    /// 非対応。
    Unsupported,
    /// その他の失敗（理由文字列）。
    Other(String),
}

/// ペアリング済み一覧の読み取り状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PairedPhase {
    /// 要求が立ち、effect ドライバへまだ渡していない（画面を開いた直後の初期値）。
    #[default]
    Requested,
    /// effect ドライバが読んでいる最中。
    Reading,
    /// 未操作・完了済み。
    Idle,
}

/// 検索の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiscoverPhase {
    /// 未操作・前回の検索が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが検索中（[`mark_discovering`] 済み）。
    Discovering,
}

/// effect ドライバへ渡す要求（世代付き）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// 完了関数へそのまま渡す世代番号。
    pub generation: u64,
}

/// Bluetooth Classic 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BluetoothClassicShowState {
    /// 一覧の読み取り状態。
    pub paired_phase: PairedPhase,
    /// 直近に立てた一覧要求の世代。
    pub paired_generation: u64,
    /// ペアリング済み機器。
    pub paired_rows: Vec<ClassicRow>,
    /// 一覧の案内（通常色）。
    pub paired_status: String,
    /// 一覧の失敗理由（`TextRole::Error`）。
    pub paired_error: String,
    /// 検索の進行状態。
    pub discover_phase: DiscoverPhase,
    /// 直近に立てた検索要求の世代。
    pub discover_generation: u64,
    /// 検索で見つかった機器。
    pub found_rows: Vec<ClassicRow>,
    /// 検索の案内（通常色）。
    pub discover_status: String,
    /// 検索の失敗理由（`TextRole::Error`）。
    pub discover_error: String,
}

impl Default for BluetoothClassicShowState {
    /// 画面を開いた最初の effect で一覧を 1 回読む。
    fn default() -> Self {
        Self {
            paired_phase: PairedPhase::Requested,
            paired_generation: 1,
            paired_rows: Vec::new(),
            paired_status: String::new(),
            paired_error: String::new(),
            discover_phase: DiscoverPhase::Idle,
            discover_generation: 0,
            found_rows: Vec::new(),
            discover_status: String::new(),
            discover_error: String::new(),
        }
    }
}

/// 「一覧を読み直す」のハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn reload_paired(state: &mut BluetoothClassicShowState) {
    if state.paired_phase != PairedPhase::Idle {
        return;
    }
    state.paired_phase = PairedPhase::Requested;
    state.paired_generation = state.paired_generation.wrapping_add(1);
    state.paired_rows.clear();
    state.paired_status = String::new();
    state.paired_error = String::new();
}

/// 「検索（12 秒）」のハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn start_discover(state: &mut BluetoothClassicShowState) {
    if state.discover_phase != DiscoverPhase::Idle {
        return;
    }
    state.discover_phase = DiscoverPhase::Requested;
    state.discover_generation = state.discover_generation.wrapping_add(1);
    state.found_rows.clear();
    state.discover_status = String::new();
    state.discover_error = String::new();
}

/// effect ドライバが未発送の一覧要求を観測する getter（状態は変えない）。
pub fn take_paired_request(state: &BluetoothClassicShowState) -> Option<Request> {
    (state.paired_phase == PairedPhase::Requested).then_some(Request {
        generation: state.paired_generation,
    })
}

/// effect ドライバが未発送の検索要求を観測する getter（状態は変えない）。
pub fn take_discover_request(state: &BluetoothClassicShowState) -> Option<Request> {
    (state.discover_phase == DiscoverPhase::Requested).then_some(Request {
        generation: state.discover_generation,
    })
}

/// 一覧要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_paired_reading(state: &mut BluetoothClassicShowState) {
    if state.paired_phase == PairedPhase::Requested {
        state.paired_phase = PairedPhase::Reading;
        state.paired_status = READING_STATUS.to_string();
    }
}

/// 検索要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_discovering(state: &mut BluetoothClassicShowState) {
    if state.discover_phase == DiscoverPhase::Requested {
        state.discover_phase = DiscoverPhase::Discovering;
        state.discover_status = DISCOVERING_STATUS.to_string();
    }
}

fn error_message(error: ClassicError) -> String {
    match error {
        ClassicError::PermissionDenied => PERMISSION_DENIED_ERROR.to_string(),
        ClassicError::Disabled => DISABLED_ERROR.to_string(),
        ClassicError::Unsupported => UNSUPPORTED_ERROR.to_string(),
        ClassicError::Other(message) => format!("{FAILED_PREFIX}{message}"),
    }
}

/// 一覧の結果を反映する。`Idle` のとき、または世代が現在と一致しないときは無視する。
/// 失敗では行を空にする（前回値を残さない）。
pub fn finish_paired(
    state: &mut BluetoothClassicShowState,
    generation: u64,
    result: Result<Vec<ClassicRow>, ClassicError>,
) {
    if state.paired_phase == PairedPhase::Idle || state.paired_generation != generation {
        return;
    }
    state.paired_phase = PairedPhase::Idle;
    state.paired_rows.clear();
    state.paired_status = String::new();
    state.paired_error = String::new();
    match result {
        Ok(rows) if rows.is_empty() => state.paired_status = PAIRED_EMPTY_STATUS.to_string(),
        Ok(rows) => {
            state.paired_status = format!("ペアリング済み {} 件", rows.len());
            state.paired_rows = rows;
        }
        Err(e) => state.paired_error = error_message(e),
    }
}

/// 検索の結果を反映する。`Idle` のとき、または世代が現在と一致しないときは無視する。
/// 失敗では行を空にする（部分結果を見せない）。
pub fn finish_discover(
    state: &mut BluetoothClassicShowState,
    generation: u64,
    result: Result<Vec<ClassicRow>, ClassicError>,
) {
    if state.discover_phase == DiscoverPhase::Idle || state.discover_generation != generation {
        return;
    }
    state.discover_phase = DiscoverPhase::Idle;
    state.found_rows.clear();
    state.discover_status = String::new();
    state.discover_error = String::new();
    match result {
        Ok(rows) if rows.is_empty() => state.discover_status = DISCOVER_EMPTY_STATUS.to_string(),
        Ok(rows) => {
            state.discover_status = format!("検索で {} 件見つかりました", rows.len());
            state.found_rows = rows;
        }
        Err(e) => state.discover_error = error_message(e),
    }
}

/// 一覧の案内（`DynamicText` の `get`）。
pub fn paired_status_text(state: &BluetoothClassicShowState) -> String {
    state.paired_status.clone()
}

/// 一覧の失敗文言（`error_text` の `get`）。
pub fn paired_error_text(state: &BluetoothClassicShowState) -> String {
    state.paired_error.clone()
}

/// 検索の案内（`DynamicText` の `get`）。
pub fn discover_status_text(state: &BluetoothClassicShowState) -> String {
    state.discover_status.clone()
}

/// 検索の失敗文言（`error_text` の `get`）。
pub fn discover_error_text(state: &BluetoothClassicShowState) -> String {
    state.discover_error.clone()
}

/// 1 行の表記（`名前 — 種別 — ペアリング済み — -60 dBm`。取れない項目は出さない）。
fn row_label(row: &ClassicRow) -> String {
    let mut parts = vec![
        row.name
            .clone()
            .unwrap_or_else(|| NAMELESS_TEXT.to_string()),
        row.kind.label().to_string(),
    ];
    if row.paired {
        parts.push(PAIRED_LABEL.to_string());
    }
    if let Some(rssi) = row.rssi {
        parts.push(format!("{rssi} dBm"));
    }
    parts.join(" — ")
}

/// `VirtualList` の 1 行あたりの固定高さ（`ble_scan::ROW_EXTENT` と同じ判断）。
const ROW_EXTENT: f32 = 24.0;

/// `Node::VirtualList::len`。一覧の行の後ろに検索の行を続ける。
fn row_count(state: &BluetoothClassicShowState) -> usize {
    state.paired_rows.len() + state.found_rows.len()
}

/// `Node::VirtualList::item`。範囲外 index は空文字列の `Text` で防御する。
fn row_node(state: &BluetoothClassicShowState, index: usize) -> Node<BluetoothClassicShowState> {
    let paired_len = state.paired_rows.len();
    let text = if index < paired_len {
        row_label(&state.paired_rows[index])
    } else {
        state
            .found_rows
            .get(index - paired_len)
            .map(|r| format!("{FOUND_PREFIX}{}", row_label(r)))
            .unwrap_or_default()
    };
    Node::Text(text)
}

/// 画面を組み立てる。ボタンは常に出し、処理中はラベルだけ変える（押しても各ハンドラが no-op）。
/// ボタンを外すと `VirtualList` の path が変わりスクロール位置が復活するため、`lan_discover` と同じ
/// 配置で path を安定させる。残り高さを消費する一覧を最後に置く。
pub fn current_screen(state: &BluetoothClassicShowState) -> Screen<BluetoothClassicShowState> {
    let discover_label = if state.discover_phase == DiscoverPhase::Idle {
        DISCOVER_BUTTON_LABEL
    } else {
        DISCOVERING_BUTTON_LABEL
    };
    Screen::new(TITLE)
        .text(NOTE)
        .dynamic_text(paired_status_text)
        .error_text(paired_error_text)
        .button(RELOAD_BUTTON_ID, RELOAD_BUTTON_LABEL, reload_paired)
        .dynamic_text(discover_status_text)
        .error_text(discover_error_text)
        .button(DISCOVER_BUTTON_ID, discover_label, start_discover)
        .virtual_list(row_count, row_node, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn row(name: Option<&str>, paired: bool, rssi: Option<i8>) -> ClassicRow {
        ClassicRow {
            name: name.map(Into::into),
            kind: ClassicKind::Phone,
            paired,
            rssi,
        }
    }

    fn discovering() -> BluetoothClassicShowState {
        let mut s = BluetoothClassicShowState::default();
        start_discover(&mut s);
        mark_discovering(&mut s);
        s
    }

    #[test]
    fn initial_state_requests_paired_read() {
        let s = BluetoothClassicShowState::default();
        assert_eq!(s.paired_phase, PairedPhase::Requested);
        assert_eq!(take_paired_request(&s), Some(Request { generation: 1 }));
        assert_eq!(take_discover_request(&s), None);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = discovering();
        finish_discover(&mut s, 99, Ok(vec![]));
        assert_eq!(s.discover_phase, DiscoverPhase::Discovering);
        let mut idle = BluetoothClassicShowState {
            paired_phase: PairedPhase::Idle,
            discover_status: "変わらない".into(),
            ..Default::default()
        };
        finish_discover(&mut idle, 0, Ok(vec![]));
        finish_paired(&mut idle, 1, Ok(vec![]));
        assert_eq!(idle.discover_status, "変わらない");
        assert_eq!(idle.paired_status, "");
    }

    #[test]
    fn empty_results_are_success_not_error() {
        let mut s = discovering();
        mark_paired_reading(&mut s);
        finish_paired(&mut s, 1, Ok(vec![]));
        finish_discover(&mut s, 1, Ok(vec![]));
        assert_eq!(s.paired_status, PAIRED_EMPTY_STATUS);
        assert_eq!(s.discover_status, DISCOVER_EMPTY_STATUS);
        assert_eq!(
            (s.paired_error.as_str(), s.discover_error.as_str()),
            ("", "")
        );
    }

    #[test]
    fn failures_are_distinct_errors_and_drop_rows() {
        let cases = [
            (
                ClassicError::PermissionDenied,
                PERMISSION_DENIED_ERROR.to_string(),
            ),
            (ClassicError::Disabled, DISABLED_ERROR.to_string()),
            (ClassicError::Unsupported, UNSUPPORTED_ERROR.to_string()),
            (ClassicError::Other("x".into()), format!("{FAILED_PREFIX}x")),
        ];
        for (err, expected) in cases {
            let mut s = discovering();
            s.found_rows = vec![row(Some("partial"), false, None)];
            finish_discover(&mut s, 1, Err(err.clone()));
            assert_eq!(s.discover_error, expected);
            assert!(s.found_rows.is_empty());
            assert_eq!(s.discover_status, "");

            let mut p = BluetoothClassicShowState {
                paired_rows: vec![row(Some("partial"), true, None)],
                ..Default::default()
            };
            finish_paired(&mut p, 1, Err(err));
            assert_eq!(p.paired_error, expected);
            assert!(p.paired_rows.is_empty());
            assert_eq!(p.paired_status, "");
        }
    }

    #[test]
    fn double_press_while_discovering_is_noop() {
        let mut s = discovering();
        assert!(click_button(
            &current_screen(&s),
            DISCOVERING_BUTTON_LABEL,
            &mut s
        ));
        assert_eq!(
            (s.discover_phase, s.discover_generation),
            (DiscoverPhase::Discovering, 1)
        );
        let mut idle = BluetoothClassicShowState::default();
        assert!(click_button(
            &current_screen(&idle),
            DISCOVER_BUTTON_LABEL,
            &mut idle
        ));
        assert_eq!(idle.discover_phase, DiscoverPhase::Requested);
    }

    #[test]
    fn reload_is_noop_until_idle() {
        let mut s = BluetoothClassicShowState::default();
        reload_paired(&mut s);
        assert_eq!(
            (s.paired_phase, s.paired_generation),
            (PairedPhase::Requested, 1)
        );
        mark_paired_reading(&mut s);
        reload_paired(&mut s);
        assert_eq!(s.paired_phase, PairedPhase::Reading);
        finish_paired(&mut s, 1, Ok(vec![]));
        reload_paired(&mut s);
        assert_eq!(
            (s.paired_phase, s.paired_generation),
            (PairedPhase::Requested, 2)
        );
    }

    #[test]
    fn paired_and_discover_generations_are_independent() {
        let mut s = discovering();
        // 一覧の完了が検索の状態を変えない。
        finish_paired(&mut s, 1, Ok(vec![row(Some("p"), true, None)]));
        assert_eq!(s.discover_phase, DiscoverPhase::Discovering);
        assert_eq!(s.discover_status, DISCOVERING_STATUS);
        // 検索の完了が一覧を変えない。
        finish_discover(&mut s, 1, Ok(vec![row(Some("f"), false, Some(-60))]));
        assert_eq!(s.paired_rows.len(), 1);
        assert_eq!(s.found_rows.len(), 1);
        assert_eq!(row_count(&s), 2);
    }

    #[test]
    fn start_discover_clears_previous_result() {
        let mut s = BluetoothClassicShowState {
            found_rows: vec![row(Some("a"), false, None)],
            discover_status: "x".into(),
            discover_error: "y".into(),
            ..Default::default()
        };
        start_discover(&mut s);
        assert!(
            s.found_rows.is_empty() && s.discover_status.is_empty() && s.discover_error.is_empty()
        );
    }

    #[test]
    fn row_label_shows_only_available_parts_and_no_address() {
        assert_eq!(
            row_label(&row(Some("Pods"), true, Some(-60))),
            "Pods — 電話 — ペアリング済み — -60 dBm"
        );
        assert_eq!(row_label(&row(None, false, None)), "（名前なし） — 電話");
    }

    #[test]
    fn found_rows_are_prefixed_after_paired_rows() {
        let s = BluetoothClassicShowState {
            paired_rows: vec![row(Some("p"), true, None)],
            found_rows: vec![row(Some("f"), false, None)],
            ..Default::default()
        };
        let Node::Text(t0) = row_node(&s, 0) else {
            panic!()
        };
        let Node::Text(t1) = row_node(&s, 1) else {
            panic!()
        };
        let Node::Text(t2) = row_node(&s, 2) else {
            panic!()
        };
        assert!(!t0.starts_with(FOUND_PREFIX));
        assert!(t1.starts_with(FOUND_PREFIX));
        assert_eq!(t2, "");
    }
}
