//! Phase 11（端末機能の拡充ループ、親 #189）型サンプル「無線の状態と切り替え」（Issue #972）。
//! Wi-Fi・Bluetooth の 2 行それぞれに「状態を読む」「オンにする」「オフにする」を置き、押すと 1 回だけ
//! OS に問い合わせ・要求して結果を出す 1 画面。`FANDHE_PROXY_APP=radio_control_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod radio_control_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの置き場」節）。
//! OS 呼び出しは `fandhe-egui-proxy` の `device/radio_control.rs` が [`take_request`] で要求を観測し
//! [`finish_read`] / [`finish_switch`] で結果を注入する（`wifi_connect_show.rs` と同型）。
//!
//! 全対象 OS（macOS / Windows / Android）の状態・文言をここで確定済みで、後続の OS 実装は本画面を
//! 触らない。ボタンは全 OS で同じで、事前判定で隠さない。起動時には何もしない。購読もしない。
//!
//! 切り替えの結果（[`Switched`]）は「結果の文言」と「読み直した状態」を並べて出す。`Accepted` /
//! `Opened` / `NotChanged` を「オン/オフになった」とは言わず、状態は読み直しの値だけで出す。
//!
//! ```text
//! Idle --ボタン--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_busy--> Busy
//!   --finish_read / finish_switch（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 画面タイトル。
pub const TITLE: &str = "無線の状態と切り替え";
/// 画面の説明。
pub const NOTE: &str = "ボタンを押すたびに 1 回だけ OS へ問い合わせ・要求します（変化の購読はしません）。切り替えの後の状態は、要求の直後に読み直した値です";
/// Wi-Fi 行の見出し。
pub const WIFI_HEADING: &str = "Wi-Fi";
/// Bluetooth 行の見出し。
pub const BLUETOOTH_HEADING: &str = "Bluetooth";
/// Wi-Fi の状態を読むボタン。
pub const WIFI_READ_LABEL: &str = "Wi-Fi の状態を読む";
/// Wi-Fi をオンにするボタン。
pub const WIFI_ON_LABEL: &str = "Wi-Fi をオンにする";
/// Wi-Fi をオフにするボタン。
pub const WIFI_OFF_LABEL: &str = "Wi-Fi をオフにする";
/// Bluetooth の状態を読むボタン。
pub const BLUETOOTH_READ_LABEL: &str = "Bluetooth の状態を読む";
/// Bluetooth をオンにするボタン。
pub const BLUETOOTH_ON_LABEL: &str = "Bluetooth をオンにする";
/// Bluetooth をオフにするボタン。
pub const BLUETOOTH_OFF_LABEL: &str = "Bluetooth をオフにする";
/// 処理中の案内。
pub const BUSY_STATUS: &str = "OS に問い合わせています…";
/// 状態の接頭辞。
pub const STATE_PREFIX: &str = "状態: ";

/// 状態の文言: オン。
pub const STATE_ON: &str = "オン";
/// 状態の文言: オフ。
pub const STATE_OFF: &str = "オフ";
/// 状態の文言: OS が止めている。
pub const STATE_DISABLED: &str = "OS が止めています（アプリからは変えられません）";
/// 状態の文言: 切り替わりの途中。
pub const STATE_TRANSITIONING: &str = "切り替わりの途中です";
/// 状態の文言: 判定できない。
pub const STATE_UNKNOWN: &str = "OS が判定できません";
/// 状態の文言: 無線がない。
pub const STATE_NOT_PRESENT: &str = "この端末にはありません";

/// 切り替え結果の文言: OS がその場で切り替えた。
pub const SWITCHED_APPLIED: &str = "切り替えました";
/// 切り替え結果の文言: OS が受け付けた。
pub const SWITCHED_ACCEPTED: &str = "OS が受け付けました";
/// 切り替え結果の文言: 設定画面を開いた。
pub const SWITCHED_OPENED: &str = "設定画面を開きました。切り替えは設定画面で行ってください";
/// 切り替え結果の文言: オンにならなかった。
pub const NOT_CHANGED_ON: &str = "オンになりませんでした";
/// 切り替え結果の文言: オフにならなかった。
pub const NOT_CHANGED_OFF: &str = "オフになりませんでした";

/// 失敗の文言: 許可がない（OS ごとの設定の場所を 1 文に並べる）。
pub const FAILED_PERMISSION: &str = "許可がありません。Android は「付近のデバイス」、Windows は設定 > プライバシーとセキュリティ > 無線 を確認してください";
/// 失敗の文言: OS が拒否した。
pub const FAILED_DENIED_BY_SYSTEM: &str = "OS が拒否しました";
/// 失敗の文言: 切り替えで非対応。
pub const FAILED_SWITCH_UNSUPPORTED: &str = "この OS ではアプリから切り替えられません";
/// 失敗の文言: 読み取りで非対応。
pub const FAILED_READ_UNSUPPORTED: &str = "この端末では無線の状態を読めません";
/// 失敗の文言: 切り替えの失敗。
pub const FAILED_SWITCH: &str = "切り替えられませんでした";
/// 失敗の文言: 読み取りの失敗。
pub const FAILED_READ: &str = "状態を読めませんでした";

const WIFI_READ_ID: WidgetId = WidgetId::from_raw(1);
const WIFI_ON_ID: WidgetId = WidgetId::from_raw(2);
const WIFI_OFF_ID: WidgetId = WidgetId::from_raw(3);
const BT_READ_ID: WidgetId = WidgetId::from_raw(4);
const BT_ON_ID: WidgetId = WidgetId::from_raw(5);
const BT_OFF_ID: WidgetId = WidgetId::from_raw(6);

/// 対象の無線。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Wi-Fi。
    Wifi,
    /// Bluetooth。
    Bluetooth,
}

/// 要求の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 状態を読む。
    Read,
    /// オン（`true`）/オフ（`false`）を要求する。
    Set(bool),
}

/// 無線の状態（`fandhe_device::radio_control::RadioState` と同じ 6 値。写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// オン。
    On,
    /// オフ。
    Off,
    /// OS が止めている。
    Disabled,
    /// 切り替わりの途中。
    Transitioning,
    /// 判定できない。
    Unknown,
    /// 無線がない。
    NotPresent,
}

/// 切り替え要求の結果（`SwitchOutcome` と同じ 4 値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switched {
    /// OS がその場で切り替えた。
    Applied,
    /// OS が受け付けた。
    Accepted,
    /// 設定画面を開いた。
    Opened,
    /// 変わらなかった。
    NotChanged,
}

/// 失敗の種別（`DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// 許可がない。
    PermissionDenied,
    /// OS が拒否した。
    DeniedBySystem,
    /// 非対応。
    Unsupported,
    /// それ以外。
    Failed,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS を呼んでいる最中（[`mark_busy`] 済み）。
    Busy,
}

/// 1 行ぶんの表示。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Row {
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。
    pub error: String,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RadioControlShowState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// effect へまだ渡していない要求。[`mark_busy`] 以降も種別の参照用に残す。
    pub pending: Option<(Kind, Action)>,
    /// Wi-Fi 行。
    pub wifi: Row,
    /// Bluetooth 行。
    pub bluetooth: Row,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestView {
    /// [`finish_read`] / [`finish_switch`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 対象の無線。
    pub kind: Kind,
    /// 要求の種類。
    pub action: Action,
}

fn row_mut(state: &mut RadioControlShowState, kind: Kind) -> &mut Row {
    match kind {
        Kind::Wifi => &mut state.wifi,
        Kind::Bluetooth => &mut state.bluetooth,
    }
}

/// ボタンの共通処理。`Idle` 以外では二重要求を防ぐため no-op。押した行の前回の結果を消す。
fn request(state: &mut RadioControlShowState, kind: Kind, action: Action) {
    if state.phase != Phase::Idle {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.pending = Some((kind, action));
    *row_mut(state, kind) = Row::default();
}

/// Wi-Fi の状態を読む。
pub fn wifi_read(state: &mut RadioControlShowState) {
    request(state, Kind::Wifi, Action::Read);
}
/// Wi-Fi をオンにする。
pub fn wifi_on(state: &mut RadioControlShowState) {
    request(state, Kind::Wifi, Action::Set(true));
}
/// Wi-Fi をオフにする。
pub fn wifi_off(state: &mut RadioControlShowState) {
    request(state, Kind::Wifi, Action::Set(false));
}
/// Bluetooth の状態を読む。
pub fn bluetooth_read(state: &mut RadioControlShowState) {
    request(state, Kind::Bluetooth, Action::Read);
}
/// Bluetooth をオンにする。
pub fn bluetooth_on(state: &mut RadioControlShowState) {
    request(state, Kind::Bluetooth, Action::Set(true));
}
/// Bluetooth をオフにする。
pub fn bluetooth_off(state: &mut RadioControlShowState) {
    request(state, Kind::Bluetooth, Action::Set(false));
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &RadioControlShowState) -> Option<RequestView> {
    if state.phase != Phase::Requested {
        return None;
    }
    state.pending.map(|(kind, action)| RequestView {
        generation: state.generation,
        kind,
        action,
    })
}

/// 処理中（`Busy`）へ進める。`Requested` 以外では何もしない。
pub fn mark_busy(state: &mut RadioControlShowState) {
    if state.phase != Phase::Requested {
        return;
    }
    state.phase = Phase::Busy;
    if let Some((kind, _)) = state.pending {
        row_mut(state, kind).status = BUSY_STATUS.to_string();
    }
}

fn reading_text(r: Reading) -> &'static str {
    match r {
        Reading::On => STATE_ON,
        Reading::Off => STATE_OFF,
        Reading::Disabled => STATE_DISABLED,
        Reading::Transitioning => STATE_TRANSITIONING,
        Reading::Unknown => STATE_UNKNOWN,
        Reading::NotPresent => STATE_NOT_PRESENT,
    }
}

fn failure_text(f: Failure, switching: bool) -> &'static str {
    match (f, switching) {
        (Failure::PermissionDenied, _) => FAILED_PERMISSION,
        (Failure::DeniedBySystem, _) => FAILED_DENIED_BY_SYSTEM,
        (Failure::Unsupported, true) => FAILED_SWITCH_UNSUPPORTED,
        (Failure::Unsupported, false) => FAILED_READ_UNSUPPORTED,
        (Failure::Failed, true) => FAILED_SWITCH,
        (Failure::Failed, false) => FAILED_READ,
    }
}

fn switched_text(s: Switched, on: bool) -> &'static str {
    match s {
        Switched::Applied => SWITCHED_APPLIED,
        Switched::Accepted => SWITCHED_ACCEPTED,
        Switched::Opened => SWITCHED_OPENED,
        Switched::NotChanged if on => NOT_CHANGED_ON,
        Switched::NotChanged => NOT_CHANGED_OFF,
    }
}

fn state_line(r: Reading) -> String {
    format!("{STATE_PREFIX}{}", reading_text(r))
}

/// 現在の世代の完了だけを受け付けて `Idle` へ戻す。古い世代・`Idle` で届いたものは `false`。
fn settle(state: &mut RadioControlShowState, generation: u64) -> bool {
    if state.phase == Phase::Idle || state.generation != generation {
        return false;
    }
    state.phase = Phase::Idle;
    state.pending = None;
    true
}

/// 状態の読み取り結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut RadioControlShowState,
    generation: u64,
    kind: Kind,
    result: Result<Reading, Failure>,
) {
    if !settle(state, generation) {
        return;
    }
    let row = row_mut(state, kind);
    *row = Row::default();
    match result {
        Ok(r) => row.status = state_line(r),
        Err(f) => row.error = failure_text(f, false).to_string(),
    }
}

/// 切り替え要求の結果と、その直後に読み直した状態を反映する。現在の世代と一致しない、または
/// `Idle` で届いた完了は無視する。`NotChanged` は失敗にせず通常色で出す。
pub fn finish_switch(
    state: &mut RadioControlShowState,
    generation: u64,
    kind: Kind,
    on: bool,
    result: Result<Switched, Failure>,
    reread: Result<Reading, Failure>,
) {
    if !settle(state, generation) {
        return;
    }
    let row = row_mut(state, kind);
    *row = Row::default();
    let mut errors = Vec::new();
    let mut lines = Vec::new();
    match result {
        Ok(s) => lines.push(switched_text(s, on).to_string()),
        Err(f) => errors.push(failure_text(f, true)),
    }
    match reread {
        Ok(r) => lines.push(state_line(r)),
        // 切り替えが失敗済みなら読み直しの失敗は重ねない（同じ原因の言い直しになるため）。
        Err(f) if errors.is_empty() => errors.push(failure_text(f, false)),
        Err(_) => {}
    }
    row.status = lines.join(" / ");
    row.error = errors.join(" / ");
}

/// Wi-Fi 行の案内文言。
pub fn wifi_status(state: &RadioControlShowState) -> String {
    state.wifi.status.clone()
}
/// Wi-Fi 行の失敗理由。
pub fn wifi_error(state: &RadioControlShowState) -> String {
    state.wifi.error.clone()
}
/// Bluetooth 行の案内文言。
pub fn bluetooth_status(state: &RadioControlShowState) -> String {
    state.bluetooth.status.clone()
}
/// Bluetooth 行の失敗理由。
pub fn bluetooth_error(state: &RadioControlShowState) -> String {
    state.bluetooth.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外して二重押しを防ぐ（ハンドラも `Idle` 以外を無視する）。
pub fn current_screen(state: &RadioControlShowState) -> Screen<RadioControlShowState> {
    let idle = state.phase == Phase::Idle;
    let mut screen = Screen::new(TITLE).text(NOTE).text(WIFI_HEADING);
    if idle {
        screen = screen
            .button(WIFI_READ_ID, WIFI_READ_LABEL, wifi_read)
            .button(WIFI_ON_ID, WIFI_ON_LABEL, wifi_on)
            .button(WIFI_OFF_ID, WIFI_OFF_LABEL, wifi_off);
    }
    screen = screen
        .dynamic_text(wifi_status)
        .error_text(wifi_error)
        .text(BLUETOOTH_HEADING);
    if idle {
        screen = screen
            .button(BT_READ_ID, BLUETOOTH_READ_LABEL, bluetooth_read)
            .button(BT_ON_ID, BLUETOOTH_ON_LABEL, bluetooth_on)
            .button(BT_OFF_ID, BLUETOOTH_OFF_LABEL, bluetooth_off);
    }
    screen
        .dynamic_text(bluetooth_status)
        .error_text(bluetooth_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn busy(kind: Kind, action: Action) -> RadioControlShowState {
        let mut s = RadioControlShowState::default();
        request(&mut s, kind, action);
        mark_busy(&mut s);
        s
    }

    #[test]
    fn startup_does_not_request() {
        let s = RadioControlShowState::default();
        assert_eq!(take_request(&s), None);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn buttons_set_expected_requests() {
        let cases: [(&str, Kind, Action); 6] = [
            (WIFI_READ_LABEL, Kind::Wifi, Action::Read),
            (WIFI_ON_LABEL, Kind::Wifi, Action::Set(true)),
            (WIFI_OFF_LABEL, Kind::Wifi, Action::Set(false)),
            (BLUETOOTH_READ_LABEL, Kind::Bluetooth, Action::Read),
            (BLUETOOTH_ON_LABEL, Kind::Bluetooth, Action::Set(true)),
            (BLUETOOTH_OFF_LABEL, Kind::Bluetooth, Action::Set(false)),
        ];
        for (label, kind, action) in cases {
            let mut s = RadioControlShowState::default();
            assert!(click_button(&current_screen(&s), label, &mut s), "{label}");
            let r = take_request(&s).expect("要求");
            assert_eq!((r.generation, r.kind, r.action), (1, kind, action));
        }
    }

    #[test]
    fn handlers_are_noop_unless_idle() {
        let mut s = busy(Kind::Wifi, Action::Read);
        wifi_on(&mut s);
        bluetooth_off(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(s.pending, Some((Kind::Wifi, Action::Read)));
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn buttons_are_hidden_while_busy() {
        let idle = current_screen(&RadioControlShowState::default()).len();
        let busy = current_screen(&busy(Kind::Wifi, Action::Read)).len();
        assert_eq!(idle, busy + 6);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = busy(Kind::Wifi, Action::Read);
        finish_read(&mut s, 99, Kind::Wifi, Ok(Reading::On));
        assert_eq!(s.phase, Phase::Busy);
        assert_eq!(s.wifi.status, BUSY_STATUS);
        let mut idle = RadioControlShowState::default();
        finish_read(&mut idle, 0, Kind::Wifi, Ok(Reading::On));
        finish_switch(
            &mut idle,
            0,
            Kind::Wifi,
            true,
            Ok(Switched::Applied),
            Ok(Reading::On),
        );
        assert_eq!(idle, RadioControlShowState::default());
    }

    #[test]
    fn new_request_clears_previous_row_only() {
        let mut s = busy(Kind::Bluetooth, Action::Read);
        finish_read(&mut s, 1, Kind::Bluetooth, Ok(Reading::On));
        wifi_read(&mut s);
        assert_eq!(s.bluetooth.status, state_line(Reading::On));
        assert_eq!(s.wifi, Row::default());
    }

    #[test]
    fn read_shows_each_state_and_never_off_for_unknown() {
        let all = [
            Reading::On,
            Reading::Off,
            Reading::Disabled,
            Reading::Transitioning,
            Reading::Unknown,
            Reading::NotPresent,
        ];
        let mut seen = std::collections::HashSet::new();
        for r in all {
            let mut s = busy(Kind::Wifi, Action::Read);
            finish_read(&mut s, 1, Kind::Wifi, Ok(r));
            assert_eq!(s.phase, Phase::Idle);
            assert_eq!(s.wifi.error, "");
            assert!(seen.insert(s.wifi.status.clone()));
            if !matches!(r, Reading::Off) {
                assert!(!s.wifi.status.contains(STATE_OFF) || r == Reading::Disabled);
            }
        }
        for r in [Reading::Unknown, Reading::NotPresent] {
            let mut s = busy(Kind::Wifi, Action::Read);
            finish_read(&mut s, 1, Kind::Wifi, Ok(r));
            assert!(!s.wifi.status.ends_with(STATE_OFF));
        }
    }

    #[test]
    fn failures_are_distinct_errors_not_success() {
        let fs = [
            Failure::PermissionDenied,
            Failure::DeniedBySystem,
            Failure::Unsupported,
            Failure::Failed,
        ];
        for switching in [false, true] {
            let mut seen = std::collections::HashSet::new();
            for f in fs {
                let mut s = busy(Kind::Wifi, Action::Set(true));
                if switching {
                    finish_switch(&mut s, 1, Kind::Wifi, true, Err(f), Err(f));
                } else {
                    finish_read(&mut s, 1, Kind::Wifi, Err(f));
                }
                assert_eq!(s.phase, Phase::Idle);
                assert!(!s.wifi.error.is_empty());
                assert_eq!(s.wifi.status, "");
                assert!(seen.insert(s.wifi.error.clone()), "文言が重複している");
                assert!(!s.wifi.error.contains(SWITCHED_APPLIED));
            }
        }
    }

    #[test]
    fn not_changed_is_normal_color_with_reread_state() {
        let mut s = busy(Kind::Bluetooth, Action::Set(true));
        finish_switch(
            &mut s,
            1,
            Kind::Bluetooth,
            true,
            Ok(Switched::NotChanged),
            Ok(Reading::Off),
        );
        assert_eq!(s.bluetooth.error, "");
        assert_eq!(
            s.bluetooth.status,
            format!("{NOT_CHANGED_ON} / {}", state_line(Reading::Off))
        );
    }

    #[test]
    fn accepted_and_opened_show_only_reread_state() {
        for (o, text) in [
            (Switched::Accepted, SWITCHED_ACCEPTED),
            (Switched::Opened, SWITCHED_OPENED),
        ] {
            let mut s = busy(Kind::Wifi, Action::Set(false));
            finish_switch(&mut s, 1, Kind::Wifi, false, Ok(o), Ok(Reading::Off));
            assert_eq!(
                s.wifi.status,
                format!("{text} / {}", state_line(Reading::Off))
            );
            let mut s = busy(Kind::Wifi, Action::Set(false));
            finish_switch(&mut s, 1, Kind::Wifi, false, Ok(o), Ok(Reading::On));
            assert!(s.wifi.status.ends_with(&state_line(Reading::On)));
        }
    }

    #[test]
    fn failed_switch_still_shows_reread_state() {
        let mut s = busy(Kind::Wifi, Action::Set(true));
        finish_switch(
            &mut s,
            1,
            Kind::Wifi,
            true,
            Err(Failure::Failed),
            Ok(Reading::Off),
        );
        assert_eq!(s.wifi.error, FAILED_SWITCH);
        assert_eq!(s.wifi.status, state_line(Reading::Off));
    }

    #[test]
    fn successful_switch_with_failed_reread_reports_read_error() {
        let mut s = busy(Kind::Wifi, Action::Set(true));
        finish_switch(
            &mut s,
            1,
            Kind::Wifi,
            true,
            Ok(Switched::Applied),
            Err(Failure::Failed),
        );
        assert_eq!(s.wifi.status, SWITCHED_APPLIED);
        assert_eq!(s.wifi.error, FAILED_READ);
    }
}
