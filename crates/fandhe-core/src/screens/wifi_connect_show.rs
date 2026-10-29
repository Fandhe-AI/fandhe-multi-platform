//! Phase 11（端末機能の拡充ループ、親 #165）型サンプル「指定した Wi-Fi への接続」（Issue #791）。
//! SSID とパスワードを入力して「接続」を押すと、その Wi-Fi への接続を 1 回要求し、OS の結果を出す
//! 1 画面。`FANDHE_PROXY_APP=wifi_connect_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod wifi_connect_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの置き場」節）。
//! OS 呼び出しは `fandhe-egui-proxy` の `device/wifi_connect.rs` が [`take_request`] で要求を観測し
//! [`finish_connect`] で結果を注入する（`wifi_info_show.rs` と同型）。
//!
//! 全対象 OS（macOS / Windows / Android）の文言・状態をここで確定済みで、後続の OS 実装は本画面を触らない。
//! 起動時には接続しない。
//!
//! # 秘密値の扱い
//!
//! パスワードは trim しない。要求を出した時点（[`connect`]）で入力欄から [`PendingConnect`] へ移して
//! 欄を空にし、effect が受け取って [`mark_connecting`] した時点で状態からも落とす。状態の `Debug` は
//! SSID・パスワードの長さだけを出す。入力欄のマスクは描画プリミティブを増やさない方針のため行わない
//! （[`NOTE`] で表示が伏せられない旨を案内する）。メモリ上の消去は保証しない。
//!
//! ```text
//! Idle --「接続」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_connecting--> Connecting
//!   --finish_connect（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 接続ボタンの `WidgetId`。
const CONNECT_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 画面タイトル。
pub const TITLE: &str = "指定した Wi-Fi への接続";
/// 画面の説明。
pub const NOTE: &str = "入力した SSID とパスワードの Wi-Fi へ 1 回接続を要求し、OS の結果を表示します。パスワードは表示が伏せられません。暗号なしの Wi-Fi はパスワードを空にしてください。今の接続は「Wi-Fi 情報」で確認できます（Android ではこのアプリだけの接続で、Wi-Fi 情報に出ないことがあります）";
/// SSID 入力欄のラベル。
pub const SSID_LABEL: &str = "SSID";
/// パスワード入力欄のラベル。
pub const PASSWORD_LABEL: &str = "パスワード（暗号なしは空欄）";
/// ボタンのラベル。
pub const CONNECT_LABEL: &str = "接続";
/// 接続を要求している間の案内。
pub const CONNECTING_STATUS: &str = "接続を要求しています…";
/// 接続できたときの案内。
pub const CONNECTED_STATUS: &str = "接続しました（この要求に対する OS の結果です）";
/// 指定の SSID が見つからないときの案内。
pub const NOT_FOUND_STATUS: &str = "指定した Wi-Fi が見つかりません";
/// Wi-Fi の電源がオフのときの案内。
pub const RADIO_OFF_STATUS: &str = "Wi-Fi がオフです";
/// 非対応のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この端末では使えません";
/// 入力不正のときの案内。
pub const INVALID_INPUT_STATUS: &str = "入力を確認してください（SSID は 1〜32 バイト、パスワードは空欄・8〜63 文字・16 進 64 文字のどれか）";
/// 許可がないときの案内（macOS）。
pub const PERMISSION_SYSTEM_SETTINGS: &str = "位置情報の許可が必要です（システム設定 > プライバシーとセキュリティ > 位置情報サービス で、位置情報サービスとこのアプリを許可してください）";
/// 許可がないときの案内（Windows）。
pub const PERMISSION_WINDOWS_SETTINGS: &str = "許可が必要です。設定 > プライバシーとセキュリティ > 位置情報 を確認して、もう一度「接続」を押してください";
/// 失敗時の案内の接頭辞（`{接頭辞}{固定文言}` の形で表示する）。
pub const FAILED_PREFIX: &str = "接続できませんでした: ";

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS へ接続を要求している最中（[`mark_connecting`] 済み）。
    Connecting,
}

/// 許可がないときの案内の出し分け（OS ごとに設定の場所が違う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionHint {
    /// macOS のシステム設定。
    SystemSettings,
    /// Windows の設定。
    WindowsSettings,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectFailure {
    /// 指定の SSID が見つからない。
    NotFound,
    /// 許可がない。
    PermissionDenied(PermissionHint),
    /// Wi-Fi の電源がオフ。
    RadioOff,
    /// 非対応。
    Unsupported,
    /// 入力が不正（OS を呼ぶ前に弾いた）。
    InvalidInput,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 結果。成功は OS がこの要求に接続したと返したことだけを表す（いま接続中だとは主張しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// OS が接続したと返した。
    Connected,
}

/// 要求を出した時点で入力欄から移した値。`Debug` は長さだけを出す。
#[derive(Clone, PartialEq, Eq)]
pub struct PendingConnect {
    generation: u64,
    ssid: String,
    password: String,
}

impl std::fmt::Debug for PendingConnect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingConnect")
            .field("generation", &self.generation)
            .field("ssid_len", &self.ssid.len())
            .field("password_len", &self.password.len())
            .finish()
    }
}

/// 接続画面の状態。`Debug` は SSID・パスワードの長さだけを出す。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct WifiConnectShowState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// SSID の入力欄。
    pub ssid: String,
    /// パスワードの入力欄（要求を出した時点で空になる）。
    pub password: String,
    /// effect へまだ渡していない要求。[`mark_connecting`] で `None` になる。
    pub pending: Option<PendingConnect>,
    /// 直近の結果。失敗・処理中は `None`。
    pub outcome: Option<Outcome>,
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空。
    pub error: String,
}

impl std::fmt::Debug for WifiConnectShowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WifiConnectShowState")
            .field("phase", &self.phase)
            .field("generation", &self.generation)
            .field("ssid_len", &self.ssid.len())
            .field("password_len", &self.password.len())
            .field("pending", &self.pending)
            .field("outcome", &self.outcome)
            .finish()
    }
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。`Debug` は長さだけを出す。
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectRequestView {
    /// [`finish_connect`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 入力された SSID（検査前の生の値）。
    pub ssid: String,
    /// 入力されたパスワード（検査前の生の値）。
    pub password: String,
}

impl std::fmt::Debug for ConnectRequestView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectRequestView")
            .field("generation", &self.generation)
            .field("ssid_len", &self.ssid.len())
            .field("password_len", &self.password.len())
            .finish()
    }
}

/// SSID 入力欄の表示値（`Screen::text_field` の getter）。
pub fn get_ssid(state: &WifiConnectShowState) -> String {
    state.ssid.clone()
}

/// SSID 入力欄の入力を反映する。処理中は no-op（要求済みの値と欄が食い違わないように）。
pub fn set_ssid(state: &mut WifiConnectShowState, value: String) {
    if state.phase == Phase::Idle {
        state.ssid = value;
    }
}

/// パスワード入力欄の表示値（`Screen::text_field` の getter）。
pub fn get_password(state: &WifiConnectShowState) -> String {
    state.password.clone()
}

/// パスワード入力欄の入力を反映する。trim しない。処理中は no-op。
pub fn set_password(state: &mut WifiConnectShowState, value: String) {
    if state.phase == Phase::Idle {
        state.password = value;
    }
}

/// ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。入力の形式は判断せず effect 側の
/// 検査に任せる。パスワード欄は要求を出した時点で空にする。
pub fn connect(state: &mut WifiConnectShowState) {
    if state.phase != Phase::Idle {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.pending = Some(PendingConnect {
        generation: state.generation,
        ssid: state.ssid.clone(),
        password: std::mem::take(&mut state.password),
    });
    // 前回の結果を残さない。
    state.outcome = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &WifiConnectShowState) -> Option<ConnectRequestView> {
    if state.phase != Phase::Requested {
        return None;
    }
    state.pending.as_ref().map(|p| ConnectRequestView {
        generation: p.generation,
        ssid: p.ssid.clone(),
        password: p.password.clone(),
    })
}

/// 接続要求中（`Connecting`）へ進める。`Requested` 以外では何もしない。状態に残る要求（パスワード）も
/// ここで落とす。
pub fn mark_connecting(state: &mut WifiConnectShowState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Connecting;
        state.pending = None;
        state.status = CONNECTING_STATUS.to_string();
    }
}

fn failure_text(f: ConnectFailure) -> String {
    match f {
        ConnectFailure::NotFound => NOT_FOUND_STATUS.to_string(),
        ConnectFailure::PermissionDenied(PermissionHint::SystemSettings) => {
            PERMISSION_SYSTEM_SETTINGS.to_string()
        }
        ConnectFailure::PermissionDenied(PermissionHint::WindowsSettings) => {
            PERMISSION_WINDOWS_SETTINGS.to_string()
        }
        ConnectFailure::RadioOff => RADIO_OFF_STATUS.to_string(),
        ConnectFailure::Unsupported => UNSUPPORTED_STATUS.to_string(),
        ConnectFailure::InvalidInput => INVALID_INPUT_STATUS.to_string(),
        ConnectFailure::Failed(m) => format!("{FAILED_PREFIX}{m}"),
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_connect(
    state: &mut WifiConnectShowState,
    generation: u64,
    result: Result<(), ConnectFailure>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.pending = None;
    state.outcome = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(()) => {
            state.outcome = Some(Outcome::Connected);
            state.status = CONNECTED_STATUS.to_string();
        }
        Err(f) => state.error = failure_text(f),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &WifiConnectShowState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &WifiConnectShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中は「接続」ボタンを外して二重押しを防ぐ（[`connect`] も `Idle` 以外を無視する）。
pub fn current_screen(state: &WifiConnectShowState) -> Screen<WifiConnectShowState> {
    let mut screen = Screen::new(TITLE)
        .text(NOTE)
        .text_field(SSID_LABEL, get_ssid, set_ssid)
        .text_field(PASSWORD_LABEL, get_password, set_password);
    if state.phase == Phase::Idle {
        screen = screen.button(CONNECT_BUTTON_ID, CONNECT_LABEL, connect);
    }
    screen.dynamic_text(status_text).error_text(error_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(ssid: &str, password: &str) -> WifiConnectShowState {
        let mut s = WifiConnectShowState::default();
        set_ssid(&mut s, ssid.to_string());
        set_password(&mut s, password.to_string());
        s
    }

    fn connecting() -> WifiConnectShowState {
        let mut s = typed("home", " pass word ");
        connect(&mut s);
        mark_connecting(&mut s);
        s
    }

    #[test]
    fn startup_does_not_request() {
        let s = WifiConnectShowState::default();
        assert_eq!(take_request(&s), None);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn connect_moves_password_to_pending_without_trim() {
        let mut s = typed(" home ", " pass word ");
        connect(&mut s);
        assert_eq!(s.password, "");
        assert_eq!(s.ssid, " home ");
        let r = take_request(&s).expect("要求");
        assert_eq!(r.generation, 1);
        assert_eq!(r.ssid, " home ");
        assert_eq!(r.password, " pass word ");
    }

    #[test]
    fn connect_is_noop_unless_idle() {
        let mut s = connecting();
        connect(&mut s);
        assert_eq!(s.phase, Phase::Connecting);
        assert_eq!(s.generation, 1);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn mark_connecting_drops_pending() {
        let s = connecting();
        assert_eq!(s.pending, None);
        assert_eq!(take_request(&s), None);
        assert_eq!(s.status, CONNECTING_STATUS);
    }

    #[test]
    fn inputs_are_ignored_while_in_flight() {
        let mut s = connecting();
        set_ssid(&mut s, "other".into());
        set_password(&mut s, "other-password".into());
        assert_eq!(s.ssid, "home");
        assert_eq!(s.password, "");
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = connecting();
        finish_connect(&mut s, 99, Ok(()));
        assert_eq!(s.phase, Phase::Connecting);
        assert_eq!(s.outcome, None);
        let mut idle = WifiConnectShowState::default();
        finish_connect(&mut idle, 0, Ok(()));
        assert_eq!(idle.outcome, None);
    }

    #[test]
    fn success_shows_connected_status_only() {
        let mut s = connecting();
        finish_connect(&mut s, 1, Ok(()));
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(s.outcome, Some(Outcome::Connected));
        assert_eq!(status_text(&s), CONNECTED_STATUS);
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn every_failure_is_distinct_error_not_success() {
        let cases = [
            (ConnectFailure::NotFound, NOT_FOUND_STATUS.to_string()),
            (
                ConnectFailure::PermissionDenied(PermissionHint::SystemSettings),
                PERMISSION_SYSTEM_SETTINGS.to_string(),
            ),
            (
                ConnectFailure::PermissionDenied(PermissionHint::WindowsSettings),
                PERMISSION_WINDOWS_SETTINGS.to_string(),
            ),
            (ConnectFailure::RadioOff, RADIO_OFF_STATUS.to_string()),
            (ConnectFailure::Unsupported, UNSUPPORTED_STATUS.to_string()),
            (
                ConnectFailure::InvalidInput,
                INVALID_INPUT_STATUS.to_string(),
            ),
            (
                ConnectFailure::Failed("x".into()),
                format!("{FAILED_PREFIX}x"),
            ),
        ];
        let mut seen = std::collections::HashSet::new();
        for (f, expected) in cases {
            let mut s = connecting();
            finish_connect(&mut s, 1, Err(f));
            assert_eq!(s.error, expected);
            assert_eq!(s.outcome, None);
            assert_eq!(s.status, "");
            assert_eq!(s.phase, Phase::Idle);
            assert!(seen.insert(expected), "文言が重複している");
        }
    }

    #[test]
    fn new_request_clears_previous_result() {
        let mut s = connecting();
        finish_connect(&mut s, 1, Ok(()));
        connect(&mut s);
        assert_eq!(s.outcome, None);
        assert_eq!(s.status, "");
        assert_eq!(s.generation, 2);
    }

    #[test]
    fn debug_hides_secrets() {
        let mut s = typed("secret-ssid", "secret-password");
        let before = format!("{s:?}");
        connect(&mut s);
        let pending = format!("{s:?} {:?}", take_request(&s));
        for d in [before, pending] {
            assert!(!d.contains("secret"), "{d}");
        }
    }

    #[test]
    fn connect_button_is_hidden_while_in_flight() {
        let idle = WifiConnectShowState::default();
        let busy = connecting();
        let n_idle = current_screen(&idle).len();
        let n_busy = current_screen(&busy).len();
        assert_eq!(n_idle, n_busy + 1);
        let mut s = typed("home", "");
        assert!(crate::click_button(
            &current_screen(&s),
            CONNECT_LABEL,
            &mut s
        ));
        assert_eq!(s.phase, Phase::Requested);
    }
}
