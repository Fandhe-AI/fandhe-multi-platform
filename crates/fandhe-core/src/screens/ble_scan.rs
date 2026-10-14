//! Phase 11 の拡充ループ型サンプル「BLE スキャン」（親 #1059、Issue #1060）。
//! 「スキャン（10 秒）」ボタン → 周辺の BLE 機器を約 10 秒スキャンし、名前と電波強度
//! （RSSI）を一覧表示する 1 画面。`FANDHE_PROXY_APP=ble_scan` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod ble_scan;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際のスキャンは本モジュール外
//! （`fandhe-egui-proxy` の effect ドライバ、`device/bluetooth_le.rs`）が
//! [`take_request`] で要求を観測し、[`finish_scan`] で結果を注入する。
//! 接続・読み書き・ペアリングは扱わない。
//!
//! # 0 台・オフ・権限拒否・非対応・キャンセルを混同しない理由
//!
//! 周辺に機器が無い（0 台）のは正常な結果で、Bluetooth がオフ・権限拒否・非搭載を
//! 「0 台」と見せると、利用者は原因を取り違える（`device-bridge-contracts.md`
//! 「6. 嘘の成功」）。0 台とキャンセルは `status`（通常色）、失敗は `error`
//! （`TextRole::Error`）へ別々に書く。失敗時は部分的な一覧を見せない（`rows` を空にする）。
//!
//! ```text
//! Idle --「スキャン」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_scanning--> phase=Scanning
//!   --finish_scan（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```
//!
//! 世代番号（`generation`）は [`take_request`]/[`finish_scan`] の照合に使い、
//! `Idle` へ戻った後や新しい要求で上書きされた古い完了を無視する
//! （`biometric_check::AuthPhase` と同じ判断）。

use crate::{Node, Screen, WidgetId};

/// 「スキャン」ボタンの `WidgetId`。
const SCAN_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// スキャン開始ボタンのラベル。
pub const SCAN_BUTTON_LABEL: &str = "スキャン（10 秒）";
/// スキャン中のボタンのラベル（押しても `start_scan` が no-op の無効表示）。
pub const SCANNING_BUTTON_LABEL: &str = "スキャン中…";
/// スキャン中の案内。
pub const SCANNING_STATUS: &str = "スキャン中…（約 10 秒）";
/// 0 台のときの案内（成功。失敗ではない）。
pub const EMPTY_STATUS: &str = "周辺に BLE 機器は見つかりませんでした";
/// キャンセルされたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// Bluetooth がオフのときの失敗文言。
pub const DISABLED_ERROR: &str = "Bluetooth がオフです";
/// 権限が拒否されたときの失敗文言。
pub const PERMISSION_DENIED_ERROR: &str = "Bluetooth の権限が拒否されています";
/// 非搭載・非対応のときの失敗文言。
pub const UNSUPPORTED_ERROR: &str = "この環境では BLE スキャンを使えません";
/// その他の失敗の接頭辞（`{接頭辞}{理由}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// RSSI が取れなかったときの表記。
pub const RSSI_UNKNOWN: &str = "不明";

/// スキャン操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバがスキャン中（[`mark_scanning`] 済み）。
    Scanning,
}

/// 一覧の 1 行（`fandhe_device::bluetooth_le::BleDevice` に対応する画面用の型。
/// `fandhe-core` は `fandhe-device` に依存しないため、写像は proxy 側の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleRow {
    /// OS が付ける識別子（名前が無いときの表示にだけ使う）。
    pub id: String,
    /// 広告された名前。無ければ `None`。
    pub name: Option<String>,
    /// 電波強度（dBm）。取れなければ `None`。
    pub rssi_dbm: Option<i16>,
}

/// スキャン失敗の種別（画面表示用。契約の `DeviceError` を 1 対 1 で写した形）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    /// Bluetooth がオフ。
    Disabled,
    /// 権限拒否。
    PermissionDenied,
    /// 非搭載・非対応。
    Unsupported,
    /// 利用者によるキャンセル（失敗として見せない）。
    Cancelled,
    /// その他の失敗（理由文字列）。
    Other(String),
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRequest {
    /// [`finish_scan`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// BLE スキャン画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BleScanState {
    /// スキャン操作の進行状態。
    pub phase: ScanPhase,
    /// 直近に立てた要求の世代番号（完了通知が「今の要求」かの照合に使う）。
    pub generation: u64,
    /// 見つかった機器（RSSI 降順で届く前提。画面側で並べ替えない）。
    pub rows: Vec<BleRow>,
    /// 0 台・キャンセル・件数・スキャン中の案内（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error`）。正常系は空文字列。
    pub error: String,
}

/// 「スキャン」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op。
pub fn start_scan(state: &mut BleScanState) {
    if state.phase != ScanPhase::Idle {
        return;
    }
    state.phase = ScanPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果を残さない。
    state.rows.clear();
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する getter（状態は変えない）。
pub fn take_request(state: &BleScanState) -> Option<ScanRequest> {
    (state.phase == ScanPhase::Requested).then_some(ScanRequest {
        generation: state.generation,
    })
}

/// 要求をスキャン中へ進める。`Requested` 以外では何もしない。
pub fn mark_scanning(state: &mut BleScanState) {
    if state.phase == ScanPhase::Requested {
        state.phase = ScanPhase::Scanning;
        state.status = SCANNING_STATUS.to_string();
    }
}

/// スキャン結果を反映する。`Idle` のとき、または世代が現在と一致しないとき
/// （新しい要求で既に上書きされた完了）は無視する。
pub fn finish_scan(
    state: &mut BleScanState,
    generation: u64,
    result: Result<Vec<BleRow>, ScanError>,
) {
    if state.phase == ScanPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ScanPhase::Idle;
    state.rows.clear();
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(rows) if rows.is_empty() => state.status = EMPTY_STATUS.to_string(),
        Ok(rows) => {
            state.status = format!("{} 台見つかりました", rows.len());
            state.rows = rows;
        }
        Err(ScanError::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Err(ScanError::Disabled) => state.error = DISABLED_ERROR.to_string(),
        Err(ScanError::PermissionDenied) => state.error = PERMISSION_DENIED_ERROR.to_string(),
        Err(ScanError::Unsupported) => state.error = UNSUPPORTED_ERROR.to_string(),
        Err(ScanError::Other(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &BleScanState) -> String {
    state.status.clone()
}

/// 失敗文言（`error_text` の `get`）。
pub fn error_text(state: &BleScanState) -> String {
    state.error.clone()
}

/// 1 行の表記（`{rssi}  {id}  {名前 または 名前なし}`）。id を名前より前に置き、
/// 省略はしない。幅が足りない場合の末尾の見切れは描画側（`VirtualList` の行）が担う。
fn row_label(row: &BleRow) -> String {
    let rssi = match row.rssi_dbm {
        Some(r) => format!("{r} dBm"),
        None => RSSI_UNKNOWN.to_string(),
    };
    // 長い名前で末尾が切られても、唯一の識別子である id が見えるよう名前の前に置く。
    format!(
        "{rssi}  {}  {}",
        row.id,
        row.name.as_deref().unwrap_or("名前なし")
    )
}

/// `VirtualList` の 1 行あたりの固定高さ（`list_detail::ROW_EXTENT` と同じ判断）。
const ROW_EXTENT: f32 = 24.0;

/// `Node::VirtualList::len`。総項目数は常に `rows.len()`。
fn row_count(state: &BleScanState) -> usize {
    state.rows.len()
}

/// `Node::VirtualList::item`。範囲外 index は空文字列の `Text` で防御する
/// （`VirtualList` は `len` の範囲内でのみ呼ぶ）。
fn row_node(state: &BleScanState, index: usize) -> Node<BleScanState> {
    Node::Text(state.rows.get(index).map(row_label).unwrap_or_default())
}

/// BLE スキャン画面を組み立てる。ボタンは常に出し、処理中はラベルだけ
/// [`SCANNING_BUTTON_LABEL`] に変える（押しても `start_scan` が no-op）。
///
/// ボタンを外すと `VirtualList` の path（ノード index）が変わり、egui の `ScrollArea` が
/// path ごとに持つスクロール位置が再スキャン後に前回の途中から復活する。path を安定させ、
/// スキャン開始で `rows` が空になる 1 フレームで位置が 0 へ丸められるようにする。
///
/// 一覧だけをスクロールさせるため、`Node::List`（egui 側でスクロールしない）ではなく
/// `VirtualList`（`ScrollArea`）を使い、残り高さを消費する一覧を最後に置く。
/// 見出し・状態文・ボタンは一覧より前に置いて固定する。
pub fn current_screen(state: &BleScanState) -> Screen<BleScanState> {
    let mut screen = Screen::new("BLE スキャン")
        .dynamic_text(status_text)
        .error_text(error_text);
    let label = if state.phase == ScanPhase::Idle {
        SCAN_BUTTON_LABEL
    } else {
        SCANNING_BUTTON_LABEL
    };
    screen = screen.button(SCAN_BUTTON_ID, label, start_scan);
    screen.virtual_list(row_count, row_node, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, name: Option<&str>, rssi: Option<i16>) -> BleRow {
        BleRow {
            id: id.into(),
            name: name.map(Into::into),
            rssi_dbm: rssi,
        }
    }

    fn scanning() -> BleScanState {
        let mut s = BleScanState::default();
        start_scan(&mut s);
        mark_scanning(&mut s);
        s
    }

    #[test]
    fn start_advances_generation_and_phase() {
        let mut s = BleScanState::default();
        start_scan(&mut s);
        assert_eq!(s.phase, ScanPhase::Requested);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn start_is_noop_while_scanning() {
        let mut s = scanning();
        start_scan(&mut s);
        assert_eq!(s.phase, ScanPhase::Scanning);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn start_clears_previous_result() {
        let mut s = BleScanState {
            rows: vec![row("a", None, None)],
            status: "x".into(),
            error: "y".into(),
            ..Default::default()
        };
        start_scan(&mut s);
        assert!(s.rows.is_empty() && s.status.is_empty() && s.error.is_empty());
    }

    #[test]
    fn take_request_only_when_requested() {
        let mut s = BleScanState::default();
        assert_eq!(take_request(&s), None);
        start_scan(&mut s);
        assert_eq!(take_request(&s), Some(ScanRequest { generation: 1 }));
        mark_scanning(&mut s);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn empty_result_is_success_not_error() {
        let mut s = scanning();
        finish_scan(&mut s, 1, Ok(vec![]));
        assert_eq!(s.phase, ScanPhase::Idle);
        assert_eq!(s.status, EMPTY_STATUS);
        assert_eq!(s.error, "");
    }

    #[test]
    fn found_devices_are_listed() {
        let mut s = scanning();
        finish_scan(&mut s, 1, Ok(vec![row("a", Some("A"), Some(-50))]));
        assert_eq!(s.status, "1 台見つかりました");
        assert_eq!(row_count(&s), 1);
    }

    #[test]
    fn failures_are_distinct_errors_without_rows_or_success_status() {
        let cases = [
            (ScanError::Disabled, DISABLED_ERROR.to_string()),
            (
                ScanError::PermissionDenied,
                PERMISSION_DENIED_ERROR.to_string(),
            ),
            (ScanError::Unsupported, UNSUPPORTED_ERROR.to_string()),
            (ScanError::Other("x".into()), format!("{FAILED_PREFIX}x")),
        ];
        for (err, expected) in cases {
            let mut s = scanning();
            s.rows = vec![row("partial", None, None)];
            finish_scan(&mut s, 1, Err(err));
            assert_eq!(s.error, expected);
            assert!(s.rows.is_empty());
            assert_eq!(s.status, "");
        }
    }

    #[test]
    fn cancelled_is_status_not_error() {
        let mut s = scanning();
        finish_scan(&mut s, 1, Err(ScanError::Cancelled));
        assert_eq!(s.status, CANCELLED_STATUS);
        assert_eq!(s.error, "");
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = scanning();
        finish_scan(&mut s, 99, Ok(vec![]));
        assert_eq!(s.phase, ScanPhase::Scanning);

        let mut idle = BleScanState {
            status: "変わらない".into(),
            ..Default::default()
        };
        finish_scan(&mut idle, 0, Ok(vec![]));
        assert_eq!(idle.status, "変わらない");
    }

    #[test]
    fn row_label_orders_rssi_name_id_without_truncation() {
        assert_eq!(row_label(&row("a", Some("A"), Some(-60))), "-60 dBm  a  A");
        assert_eq!(row_label(&row("id1", None, None)), "不明  id1  名前なし");
        let long = "あ".repeat(50);
        let label = row_label(&row("id-long", Some(&long), Some(-60)));
        assert_eq!(label, format!("-60 dBm  id-long  {long}"));
    }

    #[test]
    fn button_stays_but_is_inert_while_scanning_and_clickable_when_idle() {
        use crate::click_button;
        let mut s = scanning();
        // 初回要求 (generation=1) のまま。スキャン中のラベルを押しても二重要求にならない。
        assert!(click_button(
            &current_screen(&s),
            SCANNING_BUTTON_LABEL,
            &mut s
        ));
        assert_eq!((s.phase, s.generation), (ScanPhase::Scanning, 1));
        assert!(!click_button(
            &current_screen(&s),
            SCAN_BUTTON_LABEL,
            &mut s
        ));
        let mut idle = BleScanState::default();
        assert!(click_button(
            &current_screen(&idle),
            SCAN_BUTTON_LABEL,
            &mut idle
        ));
        assert_eq!(idle.phase, ScanPhase::Requested);
    }
}
