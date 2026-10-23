//! Phase 11 の拡充ループ型サンプル「シリアルポート」（親 #131、Issue #670）。
//! macOS・Windows ではポートを OS から列挙して一覧にし、選んだポートを固定設定で開いて
//! 固定の 1 行を送り、返ってきた 1 行を表示する。Web では利用者がブラウザのダイアログで 1 つ
//! 選び、同じ往復を行う（[`PortMode::BrowserPicker`]）。`FANDHE_PROXY_APP=serial_port_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod serial_port_show;` と対）。設定の変更・連続受信・任意文字列の
//! 送信は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/serial_port.rs`（ネイティブ）と `fandhe-web-dom`（Web）が
//! [`take_request`] で要求を観測し、[`finish_list`] / [`finish_round_trip`] で結果を注入する。
//! 状態・定数・文言は全 OS 分をここで確定し、後続の OS 実装（#671・#672）は画面に触らない。
//!
//! # 0 件・キャンセル・失敗を混同しない理由
//!
//! 0 件は正常な結果で、権限拒否・非対応を「0 件」と見せると利用者は原因を取り違える
//! （`device-bridge-contracts.md`「6. 嘘の成功」）。0 件は `status`（通常色）、失敗は `error`
//! （`TextRole::Error`）へ別々に書く。Web のダイアログを閉じたキャンセルは失敗ではなく `status`
//! に出す（同 10）。失敗とキャンセルでは前回の受信行を残さない。
//!
//! # 状態遷移
//!
//! 一覧も往復も同じ 1 本の処理（専用ワーカー）に流れるため、`phase` は 1 つだけ持つ。
//!
//! ```text
//! Idle --操作--> Requested, generation += 1
//!      --take_request/mark_running--> Running
//!      --finish_*（世代と種別が一致のみ）--> Idle
//! ```

use crate::{Node, Screen, WidgetId};

const RELOAD_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const ROUND_TRIP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const WEB_PICK_BUTTON_ID: WidgetId = WidgetId::from_raw(3);
/// 行ボタンの識別子の起点。ボタンの識別子と重ならないようにする（`list_detail.rs` と同じ流儀）。
const ROW_ID_BASE: u64 = 1000;

/// 画面タイトル。
pub const TITLE: &str = "シリアルポート";
/// 固定設定の説明（範囲の注記）。
pub const NOTE: &str =
    "9600 bps・8N1・フロー制御なしで開き、fandhe と改行を送って 1 行の返事を待つ（3 秒）";
/// 一覧の再取得ボタンのラベル。
pub const RELOAD_BUTTON_LABEL: &str = "ポートを再取得";
/// 一覧の取得中のボタンのラベル（押しても [`reload`] が no-op の無効表示）。
pub const RELOADING_BUTTON_LABEL: &str = "処理中…";
/// 往復ボタンのラベル（OS の列挙で選んだポート用）。
pub const ROUND_TRIP_BUTTON_LABEL: &str = "選んだポートで往復";
/// 往復中のボタンのラベル（無効表示）。
pub const ROUND_TRIPPING_BUTTON_LABEL: &str = "往復中…";
/// ブラウザのダイアログでポートを選ぶボタンのラベル（Web）。
pub const WEB_PICK_BUTTON_LABEL: &str = "ポートを選んで往復";
/// ダイアログ待ち・往復中のボタンのラベル（Web、無効表示）。
pub const WEB_PICKING_BUTTON_LABEL: &str = "選択・往復中…";
/// 一覧の取得中の案内。
pub const LISTING_STATUS: &str = "ポートを探しています…";
/// 往復中の案内。
pub const ROUND_TRIPPING_STATUS: &str = "往復中…（最大 3 秒）";
/// Web のダイアログ選択〜往復中の案内。
pub const WEB_PICKING_STATUS: &str = "ポートを選んでください…（選んだ後は最大 3 秒）";
/// ポートが 0 件のときの案内（成功）。
pub const EMPTY_STATUS: &str = "ポートは見つかりませんでした";
/// 未選択で往復を押したときの案内（要求は立てない）。
pub const SELECT_FIRST_STATUS: &str = "先にポートを選んでください";
/// 一覧が取れたときの案内の接尾辞（`{件数}{接尾辞}`）。
pub const FOUND_SUFFIX: &str = " 件のポートがあります（タップで選択）";
/// Web のダイアログを閉じたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "選択をやめました";
/// 受信行の接頭辞。
pub const RECEIVED_PREFIX: &str = "受信: ";
/// 空行を受けたときの表記。
pub const EMPTY_LINE_TEXT: &str = "（空行）";
/// 選択中の行に付ける印。
pub const SELECTED_MARK: &str = "▶ ";
/// 期限切れの失敗文言。
pub const TIMEOUT_ERROR: &str = "3 秒以内に返事が届きませんでした";
/// 使用中の失敗文言。
pub const BUSY_ERROR: &str = "他のアプリが使用中です";
/// 抜去・未接続の失敗文言。
pub const GONE_ERROR: &str = "ポートが見つからないか、抜かれました";
/// 権限なしの失敗文言。
pub const PERMISSION_DENIED_ERROR: &str = "シリアルポートの使用が許可されていません";
/// 非対応の失敗文言。
pub const UNSUPPORTED_ERROR: &str = "この端末ではシリアルポートを使えません";
/// 行が長すぎる失敗文言。
pub const LINE_TOO_LONG_ERROR: &str = "返事の 1 行が長すぎます（256 バイトまで）";
/// その他の失敗の接頭辞（`{接頭辞}{理由}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// ポートの選び方。マウント時に決まり、途中で変わらない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PortMode {
    /// OS が列挙したポートから選ぶ（macOS・Windows。既定）。
    #[default]
    System,
    /// ブラウザのダイアログで 1 つ選ぶ（Web）。
    BrowserPicker,
}

/// 処理の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 未操作・完了済み。
    Idle,
    /// 要求が立ち、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_running`] 済み）。
    Running,
}

/// 要求の種類。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestKind {
    /// ポートの列挙。
    List,
    /// 選んだポートでの往復。
    RoundTrip {
        /// 対象のポート名（一覧に出たもの）。
        port: String,
    },
    /// ブラウザのダイアログで選んでから往復する（Web）。
    WebPick,
}

/// effect ドライバへ渡す要求（世代付き）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// 完了関数へそのまま渡す世代番号。
    pub generation: u64,
    /// 要求の種類。
    pub kind: RequestKind,
}

/// 失敗の種別（`fandhe_device::serial_port::SerialPortFailure` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortFailure {
    /// 期限切れ。
    Timeout,
    /// 使用中。
    Busy,
    /// 抜去・未接続。
    Gone,
    /// 権限なし。
    PermissionDenied,
    /// 非対応。
    Unsupported,
    /// 受信行が長すぎる。
    LineTooLong,
    /// 利用者がダイアログを閉じた（Web のみ）。失敗ではなく案内として出す。
    Cancelled,
    /// その他の失敗（固定の理由文字列。OS の値は入れない）。
    Failed(String),
}

/// シリアルポート画面の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialPortShowState {
    /// ポートの選び方。
    pub mode: PortMode,
    /// 進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代。
    pub generation: u64,
    /// 処理中・未発送の要求の種類。`Idle` では `None`。
    pub request_kind: Option<RequestKind>,
    /// 列挙したポート名。未取得は `None`（0 件は `Some(空)`）。
    pub ports: Option<Vec<String>>,
    /// 選択中の行。
    pub selected: Option<usize>,
    /// 直近の受信行（表示用に整形済み）。
    pub received: Option<String>,
    /// 案内（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error`）。
    pub error: String,
}

impl Default for SerialPortShowState {
    /// 画面を開いた最初の effect でポートを 1 回列挙する（[`PortMode::System`]）。
    fn default() -> Self {
        Self {
            mode: PortMode::System,
            phase: Phase::Requested,
            generation: 1,
            request_kind: Some(RequestKind::List),
            ports: None,
            selected: None,
            received: None,
            status: String::new(),
            error: String::new(),
        }
    }
}

impl SerialPortShowState {
    /// Web 用の初期状態（#672 がマウントに使う）。起動時は要求を立てない。
    pub fn browser_picker() -> Self {
        Self {
            mode: PortMode::BrowserPicker,
            phase: Phase::Idle,
            generation: 0,
            request_kind: None,
            ..Self::default()
        }
    }
}

fn clear_result(state: &mut SerialPortShowState) {
    state.received = None;
    state.status = String::new();
    state.error = String::new();
}

fn request(state: &mut SerialPortShowState, kind: RequestKind) {
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.request_kind = Some(kind);
    clear_result(state);
}

/// 「ポートを再取得」のハンドラ。`Idle` 以外・Web モードでは no-op。
pub fn reload(state: &mut SerialPortShowState) {
    if state.phase != Phase::Idle || state.mode != PortMode::System {
        return;
    }
    request(state, RequestKind::List);
}

/// 行タップのハンドラ。`Idle` 以外・範囲外では選択を据え置く。
pub fn select(state: &mut SerialPortShowState, index: usize) {
    if state.phase != Phase::Idle {
        return;
    }
    if state.ports.as_ref().is_some_and(|p| index < p.len()) {
        state.selected = Some(index);
    }
}

/// 「選んだポートで往復」のハンドラ。`Idle` 以外・Web モードでは no-op。未選択なら案内だけ出して
/// 要求を立てない。
pub fn start_round_trip(state: &mut SerialPortShowState) {
    if state.phase != Phase::Idle || state.mode != PortMode::System {
        return;
    }
    let port = state
        .selected
        .and_then(|i| state.ports.as_ref()?.get(i).cloned());
    match port {
        Some(port) => request(state, RequestKind::RoundTrip { port }),
        None => {
            clear_result(state);
            state.status = SELECT_FIRST_STATUS.to_string();
        }
    }
}

/// 「ポートを選んで往復」のハンドラ（Web）。`Idle` 以外・System モードでは no-op。
pub fn start_web_pick(state: &mut SerialPortShowState) {
    if state.phase != Phase::Idle || state.mode != PortMode::BrowserPicker {
        return;
    }
    request(state, RequestKind::WebPick);
}

/// effect ドライバが未発送の要求を観測する getter（状態は変えない）。
pub fn take_request(state: &SerialPortShowState) -> Option<Request> {
    if state.phase != Phase::Requested {
        return None;
    }
    state.request_kind.clone().map(|kind| Request {
        generation: state.generation,
        kind,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut SerialPortShowState) {
    if state.phase != Phase::Requested {
        return;
    }
    state.phase = Phase::Running;
    state.status = match state.request_kind {
        Some(RequestKind::List) | None => LISTING_STATUS,
        Some(RequestKind::RoundTrip { .. }) => ROUND_TRIPPING_STATUS,
        Some(RequestKind::WebPick) => WEB_PICKING_STATUS,
    }
    .to_string();
}

/// 世代が現在と一致し、処理中で、種類が `matches` のときだけ `true`。
fn is_current(
    state: &SerialPortShowState,
    generation: u64,
    matches: impl Fn(&RequestKind) -> bool,
) -> bool {
    state.phase != Phase::Idle
        && state.generation == generation
        && state.request_kind.as_ref().is_some_and(matches)
}

/// 失敗を画面へ書く。キャンセルは失敗ではなく `status` に出す。
fn apply_failure(state: &mut SerialPortShowState, failure: PortFailure) {
    let message = match failure {
        PortFailure::Cancelled => {
            state.status = CANCELLED_STATUS.to_string();
            return;
        }
        PortFailure::Timeout => TIMEOUT_ERROR.to_string(),
        PortFailure::Busy => BUSY_ERROR.to_string(),
        PortFailure::Gone => GONE_ERROR.to_string(),
        PortFailure::PermissionDenied => PERMISSION_DENIED_ERROR.to_string(),
        PortFailure::Unsupported => UNSUPPORTED_ERROR.to_string(),
        PortFailure::LineTooLong => LINE_TOO_LONG_ERROR.to_string(),
        PortFailure::Failed(reason) => format!("{FAILED_PREFIX}{reason}"),
    };
    state.error = message;
}

/// 一覧の結果を反映する。世代・種類が一致しないものは無視する。失敗では一覧を空にして選択を外す。
/// 成功では、再取得前に選んでいたポートが残っていれば選択を保つ。
pub fn finish_list(
    state: &mut SerialPortShowState,
    generation: u64,
    result: Result<Vec<String>, PortFailure>,
) {
    if !is_current(state, generation, |k| *k == RequestKind::List) {
        return;
    }
    let previous = state
        .selected
        .and_then(|i| state.ports.as_ref()?.get(i).cloned());
    state.phase = Phase::Idle;
    state.request_kind = None;
    clear_result(state);
    state.selected = None;
    match result {
        Ok(ports) if ports.is_empty() => {
            state.status = EMPTY_STATUS.to_string();
            state.ports = Some(ports);
        }
        Ok(ports) => {
            state.selected = previous.and_then(|p| ports.iter().position(|n| *n == p));
            state.status = format!("{}{FOUND_SUFFIX}", ports.len());
            state.ports = Some(ports);
        }
        Err(failure) => {
            state.ports = None;
            apply_failure(state, failure);
        }
    }
}

/// 往復（ネイティブ）またはダイアログ選択つきの往復（Web）の結果を反映する。世代・種類が
/// 一致しないものは無視する。失敗・キャンセルでは前回の受信行を残さない。
pub fn finish_round_trip(
    state: &mut SerialPortShowState,
    generation: u64,
    result: Result<Option<String>, PortFailure>,
) {
    if !is_current(state, generation, |k| *k != RequestKind::List) {
        return;
    }
    state.phase = Phase::Idle;
    state.request_kind = None;
    clear_result(state);
    match result {
        Ok(line) => {
            state.received = Some(line.unwrap_or_else(|| EMPTY_LINE_TEXT.to_string()));
        }
        Err(failure) => apply_failure(state, failure),
    }
}

/// 案内（`DynamicText` の `get`）。
pub fn status_text(state: &SerialPortShowState) -> String {
    state.status.clone()
}

/// 失敗文言（`error_text` の `get`）。
pub fn error_text(state: &SerialPortShowState) -> String {
    state.error.clone()
}

/// 受信行（`DynamicText` の `get`）。未受信は空文字列。
pub fn received_text(state: &SerialPortShowState) -> String {
    state
        .received
        .as_ref()
        .map(|r| format!("{RECEIVED_PREFIX}{r}"))
        .unwrap_or_default()
}

/// `VirtualList` の 1 行あたりの固定高さ（`bluetooth_classic_show::ROW_EXTENT` と同じ判断）。
const ROW_EXTENT: f32 = 24.0;

fn row_count(state: &SerialPortShowState) -> usize {
    state.ports.as_ref().map_or(0, Vec::len)
}

/// `Node::VirtualList::item`。範囲外 index は空文字列の `Text` で防御する。
fn row_node(state: &SerialPortShowState, index: usize) -> Node<SerialPortShowState> {
    let Some(name) = state.ports.as_ref().and_then(|p| p.get(index)) else {
        return Node::Text(String::new());
    };
    let mark = if state.selected == Some(index) {
        SELECTED_MARK
    } else {
        ""
    };
    Node::IndexedButton {
        label: format!("{mark}{name}"),
        index,
        on_click: select,
        id: WidgetId::from_raw(ROW_ID_BASE + index as u64),
    }
}

/// 画面を組み立てる。ボタンは常に出し、処理中はラベルだけ変える（押しても各ハンドラが no-op）。
/// 残り高さを消費する一覧を最後に置く。
pub fn current_screen(state: &SerialPortShowState) -> Screen<SerialPortShowState> {
    let idle = state.phase == Phase::Idle;
    let screen = Screen::new(TITLE)
        .text(NOTE)
        .dynamic_text(status_text)
        .error_text(error_text)
        .dynamic_text(received_text);
    let screen = match state.mode {
        PortMode::System => screen
            .button(
                RELOAD_BUTTON_ID,
                if idle {
                    RELOAD_BUTTON_LABEL
                } else {
                    RELOADING_BUTTON_LABEL
                },
                reload,
            )
            .button(
                ROUND_TRIP_BUTTON_ID,
                if idle {
                    ROUND_TRIP_BUTTON_LABEL
                } else {
                    ROUND_TRIPPING_BUTTON_LABEL
                },
                start_round_trip,
            ),
        PortMode::BrowserPicker => screen.button(
            WEB_PICK_BUTTON_ID,
            if idle {
                WEB_PICK_BUTTON_LABEL
            } else {
                WEB_PICKING_BUTTON_LABEL
            },
            start_web_pick,
        ),
    };
    screen.virtual_list(row_count, row_node, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// 一覧取得済み（Idle）の System 状態。
    fn listed(v: &[&str]) -> SerialPortShowState {
        let mut s = SerialPortShowState::default();
        mark_running(&mut s);
        finish_list(&mut s, 1, Ok(names(v)));
        s
    }

    fn round_tripping() -> SerialPortShowState {
        let mut s = listed(&["/dev/cu.a", "/dev/cu.b"]);
        select(&mut s, 1);
        start_round_trip(&mut s);
        mark_running(&mut s);
        s
    }

    #[test]
    fn system_requests_list_at_start_and_browser_does_not() {
        let s = SerialPortShowState::default();
        assert_eq!(
            take_request(&s),
            Some(Request {
                generation: 1,
                kind: RequestKind::List
            })
        );
        let b = SerialPortShowState::browser_picker();
        assert_eq!(b.mode, PortMode::BrowserPicker);
        assert_eq!(take_request(&b), None);
    }

    #[test]
    fn empty_list_is_success_not_error() {
        let s = listed(&[]);
        assert_eq!(s.status, EMPTY_STATUS);
        assert_eq!(s.error, "");
        assert_eq!(s.ports, Some(vec![]));
    }

    #[test]
    fn list_failures_are_errors_and_drop_ports() {
        let cases = [
            (
                PortFailure::PermissionDenied,
                PERMISSION_DENIED_ERROR.to_string(),
            ),
            (PortFailure::Unsupported, UNSUPPORTED_ERROR.to_string()),
            (PortFailure::Gone, GONE_ERROR.to_string()),
            (PortFailure::Failed("x".into()), format!("{FAILED_PREFIX}x")),
        ];
        for (f, expected) in cases {
            let mut s = listed(&["/dev/cu.a"]);
            reload(&mut s);
            mark_running(&mut s);
            finish_list(&mut s, 2, Err(f));
            assert_eq!(s.error, expected);
            assert_eq!(s.ports, None);
            assert_eq!(s.selected, None);
            assert_eq!(s.phase, Phase::Idle);
        }
    }

    #[test]
    fn operations_are_noop_while_busy() {
        let mut s = round_tripping();
        let before = s.clone();
        reload(&mut s);
        start_round_trip(&mut s);
        start_web_pick(&mut s);
        select(&mut s, 0);
        assert_eq!(s, before);
        // ボタン経由でも同じ。
        assert!(click_button(
            &current_screen(&s),
            ROUND_TRIPPING_BUTTON_LABEL,
            &mut s
        ));
        assert_eq!(s, before);
    }

    #[test]
    fn round_trip_without_selection_shows_guidance_only() {
        let mut s = listed(&["/dev/cu.a"]);
        start_round_trip(&mut s);
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(take_request(&s), None);
        assert_eq!(s.status, SELECT_FIRST_STATUS);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn select_out_of_range_keeps_selection() {
        let mut s = listed(&["/dev/cu.a"]);
        select(&mut s, 0);
        select(&mut s, 5);
        assert_eq!(s.selected, Some(0));
        let mut none = SerialPortShowState {
            phase: Phase::Idle,
            ..Default::default()
        };
        select(&mut none, 0);
        assert_eq!(none.selected, None);
    }

    #[test]
    fn stale_generation_and_wrong_kind_are_ignored() {
        let mut s = round_tripping();
        let g = s.generation;
        finish_round_trip(&mut s, g - 1, Ok(Some("old".into())));
        assert_eq!(s.phase, Phase::Running);
        // 種類が違う完了（一覧）は往復を終わらせない。
        finish_list(&mut s, g, Ok(names(&["/dev/cu.z"])));
        assert_eq!(s.phase, Phase::Running);
        assert_eq!(s.received, None);
        finish_round_trip(&mut s, g, Ok(Some("pong".into())));
        assert_eq!(s.received.as_deref(), Some("pong"));
        // Idle になった後の完了は無視する。
        finish_round_trip(&mut s, g, Ok(Some("again".into())));
        assert_eq!(s.received.as_deref(), Some("pong"));
    }

    #[test]
    fn round_trip_result_shows_line_and_empty_line() {
        let mut s = round_tripping();
        let g = s.generation;
        finish_round_trip(&mut s, g, Ok(None));
        assert_eq!(
            received_text(&s),
            format!("{RECEIVED_PREFIX}{EMPTY_LINE_TEXT}")
        );
    }

    #[test]
    fn failures_do_not_keep_previous_line() {
        let cases = [
            (PortFailure::Timeout, TIMEOUT_ERROR),
            (PortFailure::Busy, BUSY_ERROR),
            (PortFailure::Gone, GONE_ERROR),
            (PortFailure::LineTooLong, LINE_TOO_LONG_ERROR),
        ];
        for (f, expected) in cases {
            let mut s = round_tripping();
            s.received = Some("前回".into());
            let g = s.generation;
            finish_round_trip(&mut s, g, Err(f));
            assert_eq!(s.error, expected);
            assert_eq!(s.received, None);
        }
    }

    #[test]
    fn start_clears_previous_line() {
        let mut s = round_tripping();
        let g = s.generation;
        finish_round_trip(&mut s, g, Ok(Some("pong".into())));
        start_round_trip(&mut s);
        assert_eq!(s.received, None);
        assert_eq!(s.phase, Phase::Requested);
    }

    #[test]
    fn cancelled_is_status_not_error() {
        let mut b = SerialPortShowState::browser_picker();
        start_web_pick(&mut b);
        mark_running(&mut b);
        assert_eq!(b.status, WEB_PICKING_STATUS);
        let g = b.generation;
        finish_round_trip(&mut b, g, Err(PortFailure::Cancelled));
        assert_eq!(b.status, CANCELLED_STATUS);
        assert_eq!(b.error, "");
        assert_eq!(b.received, None);
        assert_eq!(b.phase, Phase::Idle);
    }

    #[test]
    fn mode_guards_prevent_wrong_requests() {
        let mut b = SerialPortShowState::browser_picker();
        reload(&mut b);
        start_round_trip(&mut b);
        assert_eq!(take_request(&b), None);
        let mut s = listed(&["/dev/cu.a"]);
        start_web_pick(&mut s);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn reload_keeps_selection_when_port_remains() {
        let mut s = listed(&["/dev/cu.a", "/dev/cu.b"]);
        select(&mut s, 1);
        reload(&mut s);
        mark_running(&mut s);
        let g = s.generation;
        finish_list(&mut s, g, Ok(names(&["/dev/cu.b", "/dev/cu.c"])));
        assert_eq!(s.selected, Some(0));

        reload(&mut s);
        mark_running(&mut s);
        let g = s.generation;
        finish_list(&mut s, g, Ok(names(&["/dev/cu.c"])));
        assert_eq!(s.selected, None);
    }

    #[test]
    fn row_buttons_select_and_mark() {
        let mut s = listed(&["/dev/cu.a", "/dev/cu.b"]);
        assert_eq!(row_count(&s), 2);
        // `VirtualList` の子は `click_button` では解決できないため、行ノードのハンドラを直接呼ぶ。
        let Node::IndexedButton {
            index, on_click, ..
        } = row_node(&s, 1)
        else {
            panic!()
        };
        on_click(&mut s, index);
        assert_eq!(s.selected, Some(1));
        let Node::IndexedButton { label, .. } = row_node(&s, 1) else {
            panic!()
        };
        assert_eq!(label, format!("{SELECTED_MARK}/dev/cu.b"));
        let Node::Text(t) = row_node(&s, 9) else {
            panic!()
        };
        assert_eq!(t, "");
    }

    #[test]
    fn mark_running_picks_status_per_kind() {
        let mut s = SerialPortShowState::default();
        mark_running(&mut s);
        assert_eq!(s.status, LISTING_STATUS);
        let s = round_tripping();
        assert_eq!(s.status, ROUND_TRIPPING_STATUS);
    }

    #[test]
    fn take_request_carries_selected_port() {
        let mut s = listed(&["/dev/cu.a", "/dev/cu.b"]);
        select(&mut s, 1);
        start_round_trip(&mut s);
        assert_eq!(
            take_request(&s).map(|r| r.kind),
            Some(RequestKind::RoundTrip {
                port: "/dev/cu.b".into()
            })
        );
    }
}
