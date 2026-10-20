//! Phase 11 の拡充ループ型サンプル「LAN のサービス探索」（親 #109、Issue #486）。
//! 「探索（10 秒）」ボタン → 同じネットワークで `_http._tcp`（mDNS / Bonjour）を公開している
//! 機器を約 10 秒探し、名前とアドレスを一覧表示する 1 画面。`FANDHE_PROXY_APP=lan_discover` で
//! 選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod lan_discover;` と対）。接続・公開は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の探索は本モジュール外
//! （`fandhe-egui-proxy` の effect ドライバ、`device/lan_discovery.rs`）が
//! [`take_request`] で要求を観測し、[`finish_browse`] で結果を注入する。状態・定数・文言は
//! 全 OS 分をここで確定し、後続の OS 実装は画面に触らない。
//!
//! # 0 件・未接続・許可なし・非対応を混同しない理由
//!
//! 同じネットワークに該当サービスが無い（0 件）のは正常な結果で、未接続・許可なし・非対応を
//! 「0 件」と見せると利用者は原因を取り違える（`device-bridge-contracts.md`「6. 嘘の成功」）。
//! 0 件は `status`（通常色）、失敗は `error`（`TextRole::Error`）へ別々に書く。失敗時は部分的な
//! 一覧を見せない（`rows` を空にする）。利用者が取り消す操作は無いため `Cancelled` は持たない。
//!
//! ```text
//! Idle --「探索」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_browsing--> phase=Browsing
//!   --finish_browse（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```

use crate::{Node, Screen, WidgetId};

/// 「探索」ボタンの `WidgetId`。
const BROWSE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 探索開始ボタンのラベル。
pub const BROWSE_BUTTON_LABEL: &str = "探索（10 秒）";
/// 探索中のボタンのラベル（押しても `start_browse` が no-op の無効表示）。
pub const BROWSING_BUTTON_LABEL: &str = "探索中…";
/// 探索中の案内。
pub const BROWSING_STATUS: &str = "探索中…（約 10 秒）";
/// 0 件のときの案内（成功。失敗ではない）。
pub const EMPTY_STATUS: &str = "同じネットワークに http サービスは見つかりませんでした";
/// ローカルネットワークの許可がないときの失敗文言。
pub const PERMISSION_DENIED_ERROR: &str = "ローカルネットワークへのアクセスが許可されていません";
/// ネットワーク未接続のときの失敗文言。
pub const DISABLED_ERROR: &str = "ネットワークに接続されていません";
/// 非対応のときの失敗文言。
pub const UNSUPPORTED_ERROR: &str = "この端末ではサービス探索を使えません";
/// その他の失敗の接頭辞（`{接頭辞}{理由}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// アドレスが解決できなかった行の表記。
pub const UNRESOLVED: &str = "アドレス未解決";
/// 画面の注記（探索の範囲）。
pub const NOTE: &str = "同じネットワークで _http._tcp を公開している機器を 10 秒探し、名前とアドレスを出します（接続はしません）";

/// 探索操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LanPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが探索中（[`mark_browsing`] 済み）。
    Browsing,
}

/// 一覧の 1 行（`fandhe_device::lan_discovery::LanService` に対応する画面用の型。
/// `fandhe-core` は `fandhe-device` に依存しないため、`IpAddr` の文字列化は proxy 側の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanRow {
    /// サービス名。
    pub name: String,
    /// IP アドレスの文字列。未解決は `None`。
    pub addr: Option<String>,
    /// ポート。未解決は `None`。
    pub port: Option<u16>,
}

/// 探索失敗の種別（画面表示用。契約の `DeviceError` を写した形）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanError {
    /// ローカルネットワークの許可なし。
    PermissionDenied,
    /// ネットワーク未接続。
    Disabled,
    /// 非対応。
    Unsupported,
    /// その他の失敗（理由文字列）。
    Other(String),
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseRequest {
    /// [`finish_browse`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// LAN のサービス探索画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LanDiscoverState {
    /// 探索操作の進行状態。
    pub phase: LanPhase,
    /// 直近に立てた要求の世代番号（完了通知が「今の要求」かの照合に使う）。
    pub generation: u64,
    /// 見つかったサービス（名前昇順で届く前提。画面側で並べ替えない）。
    pub rows: Vec<LanRow>,
    /// 0 件・件数・探索中の案内（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error`）。正常系は空文字列。
    pub error: String,
}

/// 「探索」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op。
pub fn start_browse(state: &mut LanDiscoverState) {
    if state.phase != LanPhase::Idle {
        return;
    }
    state.phase = LanPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果を残さない。
    state.rows.clear();
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する getter（状態は変えない）。
pub fn take_request(state: &LanDiscoverState) -> Option<BrowseRequest> {
    (state.phase == LanPhase::Requested).then_some(BrowseRequest {
        generation: state.generation,
    })
}

/// 要求を探索中へ進める。`Requested` 以外では何もしない。
pub fn mark_browsing(state: &mut LanDiscoverState) {
    if state.phase == LanPhase::Requested {
        state.phase = LanPhase::Browsing;
        state.status = BROWSING_STATUS.to_string();
    }
}

/// 探索結果を反映する。`Idle` のとき、または世代が現在と一致しないとき
/// （新しい要求で既に上書きされた完了）は無視する。
pub fn finish_browse(
    state: &mut LanDiscoverState,
    generation: u64,
    result: Result<Vec<LanRow>, LanError>,
) {
    if state.phase == LanPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = LanPhase::Idle;
    state.rows.clear();
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(rows) if rows.is_empty() => state.status = EMPTY_STATUS.to_string(),
        Ok(rows) => {
            state.status = format!("{} 件見つかりました", rows.len());
            state.rows = rows;
        }
        Err(LanError::PermissionDenied) => state.error = PERMISSION_DENIED_ERROR.to_string(),
        Err(LanError::Disabled) => state.error = DISABLED_ERROR.to_string(),
        Err(LanError::Unsupported) => state.error = UNSUPPORTED_ERROR.to_string(),
        Err(LanError::Other(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &LanDiscoverState) -> String {
    state.status.clone()
}

/// 失敗文言（`error_text` の `get`）。
pub fn error_text(state: &LanDiscoverState) -> String {
    state.error.clone()
}

/// 1 行の表記（`名前 — IP:ポート`。IPv6 は `[addr]:port`。未解決は `名前 — アドレス未解決`）。
fn row_label(row: &LanRow) -> String {
    let target = match (&row.addr, row.port) {
        (Some(addr), Some(port)) if addr.contains(':') => format!("[{addr}]:{port}"),
        (Some(addr), Some(port)) => format!("{addr}:{port}"),
        (Some(addr), None) => addr.clone(),
        (None, _) => UNRESOLVED.to_string(),
    };
    format!("{} — {target}", row.name)
}

/// `VirtualList` の 1 行あたりの固定高さ（`ble_scan::ROW_EXTENT` と同じ判断）。
const ROW_EXTENT: f32 = 24.0;

/// `Node::VirtualList::len`。総項目数は常に `rows.len()`。
fn row_count(state: &LanDiscoverState) -> usize {
    state.rows.len()
}

/// `Node::VirtualList::item`。範囲外 index は空文字列の `Text` で防御する。
fn row_node(state: &LanDiscoverState, index: usize) -> Node<LanDiscoverState> {
    Node::Text(state.rows.get(index).map(row_label).unwrap_or_default())
}

/// LAN のサービス探索画面を組み立てる。ボタンは常に出し、処理中はラベルだけ
/// [`BROWSING_BUTTON_LABEL`] に変える（押しても `start_browse` が no-op）。ボタンを外すと
/// `VirtualList` の path が変わりスクロール位置が復活するため、`ble_scan` と同じ配置で path を
/// 安定させる。残り高さを消費する一覧を最後に置く。
pub fn current_screen(state: &LanDiscoverState) -> Screen<LanDiscoverState> {
    let mut screen = Screen::new("LAN のサービス探索")
        .text(NOTE)
        .dynamic_text(status_text)
        .error_text(error_text);
    let label = if state.phase == LanPhase::Idle {
        BROWSE_BUTTON_LABEL
    } else {
        BROWSING_BUTTON_LABEL
    };
    screen = screen.button(BROWSE_BUTTON_ID, label, start_browse);
    screen.virtual_list(row_count, row_node, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, addr: Option<&str>, port: Option<u16>) -> LanRow {
        LanRow {
            name: name.into(),
            addr: addr.map(Into::into),
            port,
        }
    }

    fn browsing() -> LanDiscoverState {
        let mut s = LanDiscoverState::default();
        start_browse(&mut s);
        mark_browsing(&mut s);
        s
    }

    #[test]
    fn start_advances_generation_and_clears_previous_result() {
        let mut s = LanDiscoverState {
            rows: vec![row("a", None, None)],
            status: "x".into(),
            error: "y".into(),
            ..Default::default()
        };
        start_browse(&mut s);
        assert_eq!((s.phase, s.generation), (LanPhase::Requested, 1));
        assert!(s.rows.is_empty() && s.status.is_empty() && s.error.is_empty());
    }

    #[test]
    fn take_request_is_none_while_browsing() {
        let mut s = LanDiscoverState::default();
        assert_eq!(take_request(&s), None);
        start_browse(&mut s);
        assert_eq!(take_request(&s), Some(BrowseRequest { generation: 1 }));
        mark_browsing(&mut s);
        assert_eq!(take_request(&s), None);
        start_browse(&mut s);
        assert_eq!((s.phase, s.generation), (LanPhase::Browsing, 1));
    }

    #[test]
    fn empty_result_is_success_not_error() {
        let mut s = browsing();
        finish_browse(&mut s, 1, Ok(vec![]));
        assert_eq!(s.phase, LanPhase::Idle);
        assert_eq!(s.status, EMPTY_STATUS);
        assert_eq!(s.error, "");
    }

    #[test]
    fn failures_are_distinct_errors_and_drop_rows() {
        let cases = [
            (
                LanError::PermissionDenied,
                PERMISSION_DENIED_ERROR.to_string(),
            ),
            (LanError::Disabled, DISABLED_ERROR.to_string()),
            (LanError::Unsupported, UNSUPPORTED_ERROR.to_string()),
            (LanError::Other("x".into()), format!("{FAILED_PREFIX}x")),
        ];
        for (err, expected) in cases {
            let mut s = browsing();
            s.rows = vec![row("partial", None, None)];
            finish_browse(&mut s, 1, Err(err));
            assert_eq!(s.error, expected);
            assert!(s.rows.is_empty());
            assert_eq!(s.status, "");
        }
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = browsing();
        finish_browse(&mut s, 99, Ok(vec![]));
        assert_eq!(s.phase, LanPhase::Browsing);
        let mut idle = LanDiscoverState {
            status: "変わらない".into(),
            ..Default::default()
        };
        finish_browse(&mut idle, 0, Ok(vec![]));
        assert_eq!(idle.status, "変わらない");
    }

    #[test]
    fn found_services_are_listed_and_unresolved_row_is_kept() {
        let mut s = browsing();
        finish_browse(
            &mut s,
            1,
            Ok(vec![
                row("a", Some("10.0.0.2"), Some(80)),
                row("b", None, None),
            ]),
        );
        assert_eq!(s.status, "2 件見つかりました");
        assert_eq!(row_count(&s), 2);
        assert_eq!(row_label(&s.rows[1]), "b — アドレス未解決");
    }

    #[test]
    fn row_label_formats_ipv4_ipv6_and_missing_port() {
        assert_eq!(
            row_label(&row("a", Some("10.0.0.2"), Some(80))),
            "a — 10.0.0.2:80"
        );
        assert_eq!(
            row_label(&row("a", Some("fe80::1"), Some(80))),
            "a — [fe80::1]:80"
        );
        assert_eq!(row_label(&row("a", Some("10.0.0.2"), None)), "a — 10.0.0.2");
    }

    #[test]
    fn button_is_inert_while_browsing_and_clickable_when_idle() {
        use crate::click_button;
        let mut s = browsing();
        assert!(click_button(
            &current_screen(&s),
            BROWSING_BUTTON_LABEL,
            &mut s
        ));
        assert_eq!((s.phase, s.generation), (LanPhase::Browsing, 1));
        let mut idle = LanDiscoverState::default();
        assert!(click_button(
            &current_screen(&idle),
            BROWSE_BUTTON_LABEL,
            &mut idle
        ));
        assert_eq!(idle.phase, LanPhase::Requested);
    }
}
