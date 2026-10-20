//! Phase 11（端末機能の拡充ループ、親 #111）型サンプル「Wi-Fi 情報」（Issue #514）。
//! 「読む」ボタン → 今つないでいる Wi-Fi の名前（SSID）と電波強度（dBm）、OS が持つ直近のスキャン結果
//! から周辺のネットワークを強い順に 5 件まで出す 1 画面。`FANDHE_PROXY_APP=wifi_info_show` で
//! 選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod wifi_info_show;` と対）。自分からのスキャン・BSSID は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の `device/wifi_info.rs` が
//! [`take_request`] で要求を観測し [`finish_read`] で結果を注入する（`music_library_list.rs` と同型）。
//!
//! 全対象 OS（macOS / Windows / Android）の文言・状態をここで確定済みで、後続の OS 実装は本画面を
//! 触らない。起動時には読まない（許可ダイアログを開いただけで出さないため）。
//!
//! # 0 件と失敗の区別
//!
//! 未接続かつ周辺 0 件は成功で、失敗の文言とは別にする。失敗時は前回の結果も 0 件の文言も残さない
//! （`device-bridge-contracts.md`「6. 嘘の成功」）。SSID は位置を推定できるため、状態の `Debug` は
//! 件数と接続の有無だけを出す。
//!
//! ```text
//! Idle --「読む」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_loading--> Loading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Node, Screen, WidgetId};

/// 読み取りボタンの `WidgetId`。
const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// `VirtualList` の 1 行あたりの固定高さ。
const ROW_EXTENT: f32 = 24.0;

/// 画面タイトル。
pub const TITLE: &str = "Wi-Fi 情報";
/// 画面の説明。
pub const NOTE: &str = "今つないでいる Wi-Fi と、OS が持っている直近のスキャン結果から周辺のネットワークを強い順に 5 件まで表示します（自分からスキャンはしません）";
/// ボタンのラベル。
pub const READ_LABEL: &str = "読む";
/// 読み込み中の案内。
pub const LOADING_STATUS: &str = "読み込み中…";
/// 未接続のときの案内（失敗ではない）。
pub const DISCONNECTED_TEXT: &str = "未接続です";
/// SSID が取れない（非公開・非 UTF-8）ときの代替表示。
pub const HIDDEN_SSID: &str = "非公開";
/// 電波強度が不明なときの代替表示。
pub const UNKNOWN_STRENGTH: &str = "強度不明";
/// 周辺の見出し。
pub const NEARBY_HEADING: &str = "周辺のネットワーク（強い順）";
/// 周辺が 0 件だったときの案内（失敗ではない）。
pub const NEARBY_EMPTY: &str = "周辺のネットワークはありません（OS の直近のスキャン結果）";
/// 非対応のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この端末では使えません";
/// Wi-Fi の電源がオフのときの案内。
pub const RADIO_OFF_STATUS: &str = "Wi-Fi がオフです";
/// 位置情報サービスがオフのときの案内。
pub const LOCATION_OFF_STATUS: &str =
    "位置情報サービスがオフです（Wi-Fi の名前を読むのに必要です）";
/// 許可がないときの案内（macOS）。
pub const PERMISSION_SYSTEM_SETTINGS: &str =
    "位置情報の許可が必要です（システム設定 > プライバシーとセキュリティ > 位置情報サービス）";
/// 許可がないときの案内（Windows）。
pub const PERMISSION_WINDOWS_SETTINGS: &str = "位置情報の許可が必要です。設定 > プライバシーとセキュリティ > 位置情報 で許可して、もう一度「読む」を押してください";
/// 許可がないときの案内（Android）。
pub const PERMISSION_ANDROID_PRECISE: &str =
    "正確な位置情報の許可が必要です（「おおよそ」では読めません）";
/// 失敗時の案内の接頭辞（`{接頭辞}{固定文言}` の形で表示する）。
pub const FAILED_PREFIX: &str = "読めませんでした: ";

/// 表示用の 1 行。`ssid` が `None` なら [`HIDDEN_SSID`]、`rssi_dbm` が `None` なら [`UNKNOWN_STRENGTH`]。
#[derive(Clone, PartialEq, Eq)]
pub struct WifiRow {
    /// 名前。
    pub ssid: Option<String>,
    /// 電波強度（dBm）。
    pub rssi_dbm: Option<i16>,
}

/// 読み取れた結果。
///
/// `Debug` は SSID を出さない（接続の有無と件数のみ。ログ・panic 経由の漏えい防止）。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct WifiView {
    /// 現在の接続。未接続は `None`。
    pub current: Option<WifiRow>,
    /// 周辺（強い順、5 件まで）。
    pub nearby: Vec<WifiRow>,
}

impl std::fmt::Debug for WifiView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WifiView")
            .field("connected", &self.current.is_some())
            .field("nearby", &self.nearby.len())
            .finish()
    }
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS から読んでいる最中（[`mark_loading`] 済み）。
    Loading,
}

/// 許可がないときの案内の出し分け（OS ごとに設定の場所が違う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionHint {
    /// macOS のシステム設定。
    SystemSettings,
    /// Windows の設定。
    WindowsSettings,
    /// Android の「正確な位置情報」。
    AndroidPrecise,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WifiFailure {
    /// 非対応。
    Unsupported,
    /// Wi-Fi の電源がオフ。
    RadioOff,
    /// 位置情報サービスがオフ。
    LocationOff,
    /// 位置情報の許可がない。
    PermissionDenied(PermissionHint),
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// Wi-Fi 情報画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WifiInfoShowState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読み取れた結果。失敗・読み込み中は `None`（未接続かつ周辺 0 件は `Some` の空）。
    pub view: Option<WifiView>,
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

/// ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn fetch(state: &mut WifiInfoShowState) {
    if state.phase != Phase::Idle {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果を残さない。
    state.view = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &WifiInfoShowState) -> Option<ReadRequest> {
    (state.phase == Phase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 読み込み中（`Loading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_loading(state: &mut WifiInfoShowState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Loading;
        state.status = LOADING_STATUS.to_string();
    }
}

fn failure_text(f: WifiFailure) -> String {
    match f {
        WifiFailure::Unsupported => UNSUPPORTED_STATUS.to_string(),
        WifiFailure::RadioOff => RADIO_OFF_STATUS.to_string(),
        WifiFailure::LocationOff => LOCATION_OFF_STATUS.to_string(),
        WifiFailure::PermissionDenied(PermissionHint::SystemSettings) => {
            PERMISSION_SYSTEM_SETTINGS.to_string()
        }
        WifiFailure::PermissionDenied(PermissionHint::WindowsSettings) => {
            PERMISSION_WINDOWS_SETTINGS.to_string()
        }
        WifiFailure::PermissionDenied(PermissionHint::AndroidPrecise) => {
            PERMISSION_ANDROID_PRECISE.to_string()
        }
        WifiFailure::Failed(m) => format!("{FAILED_PREFIX}{m}"),
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut WifiInfoShowState,
    generation: u64,
    result: Result<WifiView, WifiFailure>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.view = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(v) => state.view = Some(v),
        Err(f) => state.error = failure_text(f),
    }
}

fn strength(rssi: Option<i16>) -> String {
    rssi.map_or_else(|| UNKNOWN_STRENGTH.to_string(), |d| format!("{d} dBm"))
}

fn ssid_or_hidden(ssid: &Option<String>) -> &str {
    ssid.as_deref().unwrap_or(HIDDEN_SSID)
}

/// 案内文言（`DynamicText` の `get`）。読み込み中は案内、成功時は接続の有無。
pub fn status_text(state: &WifiInfoShowState) -> String {
    match &state.view {
        Some(WifiView {
            current: Some(c), ..
        }) => format!(
            "接続中: {}（{}）",
            ssid_or_hidden(&c.ssid),
            strength(c.rssi_dbm)
        ),
        Some(_) => DISCONNECTED_TEXT.to_string(),
        None => state.status.clone(),
    }
}

/// 周辺の見出し（成功時のみ。0 件は失敗ではない旨の案内）。
pub fn nearby_heading(state: &WifiInfoShowState) -> String {
    match &state.view {
        Some(v) if v.nearby.is_empty() => NEARBY_EMPTY.to_string(),
        Some(_) => NEARBY_HEADING.to_string(),
        None => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &WifiInfoShowState) -> String {
    state.error.clone()
}

/// `VirtualList` の総行数。
pub fn row_count(state: &WifiInfoShowState) -> usize {
    state.view.as_ref().map_or(0, |v| v.nearby.len())
}

/// 1 行の表示文字列（「SSID — dBm」）。範囲外は panic せず空。
pub fn row_text(state: &WifiInfoShowState, index: usize) -> String {
    state
        .view
        .as_ref()
        .and_then(|v| v.nearby.get(index))
        .map(|r| format!("{} — {}", ssid_or_hidden(&r.ssid), strength(r.rssi_dbm)))
        .unwrap_or_default()
}

/// `VirtualList` の 1 行。
pub fn nearby_row(state: &WifiInfoShowState, index: usize) -> Node<WifiInfoShowState> {
    Node::Text(row_text(state, index))
}

/// 画面を組み立てる。ボタンは常に出す（隠すと `VirtualList` のノードパスが変わりスクロール位置が
/// 残るため。`music_library_list` と同じ）。処理中の押下は [`fetch`] が無視する。
pub fn current_screen(_state: &WifiInfoShowState) -> Screen<WifiInfoShowState> {
    Screen::new(TITLE)
        .text(NOTE)
        .button(READ_BUTTON_ID, READ_LABEL, fetch)
        .dynamic_text(status_text)
        .error_text(error_text)
        .dynamic_text(nearby_heading)
        .virtual_list(row_count, nearby_row, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn row(name: Option<&str>, rssi: Option<i16>) -> WifiRow {
        WifiRow {
            ssid: name.map(str::to_string),
            rssi_dbm: rssi,
        }
    }

    fn loading() -> WifiInfoShowState {
        let mut s = WifiInfoShowState::default();
        fetch(&mut s);
        mark_loading(&mut s);
        s
    }

    #[test]
    fn startup_does_not_request() {
        let s = WifiInfoShowState::default();
        assert_eq!(take_request(&s), None);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn fetch_is_noop_while_in_flight() {
        let mut s = WifiInfoShowState::default();
        fetch(&mut s);
        assert_eq!(take_request(&s), Some(ReadRequest { generation: 1 }));
        mark_loading(&mut s);
        assert_eq!(take_request(&s), None);
        fetch(&mut s);
        assert_eq!(s.phase, Phase::Loading);
        assert_eq!(s.generation, 1);
        assert_eq!(s.status, LOADING_STATUS);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = loading();
        finish_read(&mut s, 99, Ok(WifiView::default()));
        assert_eq!(s.phase, Phase::Loading);
        assert_eq!(s.view, None);
        let mut idle = WifiInfoShowState::default();
        finish_read(&mut idle, 0, Ok(WifiView::default()));
        assert_eq!(idle.view, None);
    }

    #[test]
    fn disconnected_and_empty_nearby_is_success_distinct_from_failure() {
        let mut s = loading();
        finish_read(&mut s, 1, Ok(WifiView::default()));
        assert_eq!(s.view, Some(WifiView::default()));
        assert_eq!(status_text(&s), DISCONNECTED_TEXT);
        assert_eq!(nearby_heading(&s), NEARBY_EMPTY);
        assert_eq!(s.error, "");
    }

    #[test]
    fn every_failure_is_distinct_and_leaves_no_result() {
        let cases = [
            (WifiFailure::Unsupported, UNSUPPORTED_STATUS.to_string()),
            (WifiFailure::RadioOff, RADIO_OFF_STATUS.to_string()),
            (WifiFailure::LocationOff, LOCATION_OFF_STATUS.to_string()),
            (
                WifiFailure::PermissionDenied(PermissionHint::SystemSettings),
                PERMISSION_SYSTEM_SETTINGS.to_string(),
            ),
            (
                WifiFailure::PermissionDenied(PermissionHint::WindowsSettings),
                PERMISSION_WINDOWS_SETTINGS.to_string(),
            ),
            (
                WifiFailure::PermissionDenied(PermissionHint::AndroidPrecise),
                PERMISSION_ANDROID_PRECISE.to_string(),
            ),
            (WifiFailure::Failed("x".into()), format!("{FAILED_PREFIX}x")),
        ];
        let mut seen = std::collections::HashSet::new();
        for (f, expected) in cases {
            let mut s = loading();
            s.view = Some(WifiView {
                current: Some(row(Some("a"), Some(-40))),
                nearby: vec![row(Some("b"), Some(-50))],
            });
            finish_read(&mut s, 1, Err(f));
            assert_eq!(s.phase, Phase::Idle);
            assert_eq!(s.view, None);
            assert_eq!(status_text(&s), "");
            assert_eq!(nearby_heading(&s), "");
            assert_eq!(row_count(&s), 0);
            assert_eq!(s.error, expected);
            assert!(seen.insert(expected), "文言が重複");
        }
    }

    #[test]
    fn hidden_ssid_and_unknown_strength_placeholders() {
        let mut s = loading();
        finish_read(
            &mut s,
            1,
            Ok(WifiView {
                current: Some(row(None, None)),
                nearby: vec![row(Some("n"), Some(-60)), row(None, None)],
            }),
        );
        assert_eq!(
            status_text(&s),
            format!("接続中: {HIDDEN_SSID}（{UNKNOWN_STRENGTH}）")
        );
        assert_eq!(nearby_heading(&s), NEARBY_HEADING);
        assert_eq!(row_text(&s, 0), "n — -60 dBm");
        assert_eq!(
            row_text(&s, 1),
            format!("{HIDDEN_SSID} — {UNKNOWN_STRENGTH}")
        );
        assert_eq!(row_text(&s, 9), "");
    }

    #[test]
    fn debug_output_hides_ssid() {
        let s = WifiInfoShowState {
            view: Some(WifiView {
                current: Some(row(Some("secret-home"), Some(-40))),
                nearby: vec![row(Some("neighbor"), Some(-50))],
            }),
            ..Default::default()
        };
        let out = format!("{s:?}");
        assert!(
            !out.contains("secret") && !out.contains("neighbor"),
            "{out}"
        );
    }

    #[test]
    fn button_click_requests() {
        let mut s = WifiInfoShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, READ_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Requested);
    }
}
