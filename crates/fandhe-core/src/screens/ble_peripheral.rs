//! Phase 11（端末機能の拡充ループ、親 #164）型サンプル「BLE の周辺機器役」（Issue #778）。
//! 「公開」ボタンで固定のサービス UUID と読み取り専用の特性を登録してアドバタイズし、
//! 今回の公開で読み取られた回数を画面に出す。「停止」ボタン・アプリ終了・iOS の背景遷移で止める。
//! `FANDHE_PROXY_APP=ble_peripheral` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod ble_peripheral;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。OS への依頼・事象の受け取りは `fandhe-egui-proxy` の effect ドライバ
//! （`device/ble_peripheral.rs`）が [`take_action`] で要求を観測して行い、[`finish_start`] /
//! [`finish_stop`] / [`apply_event`] / [`apply_reads`] で結果を注入する。本画面は二重押し防止・
//! 世代の照合・結果の種類ごとの文言だけを持つ。全対象 OS 分（iOS の背景遷移による中断を含む）の
//! 状態・文言を本ファイルで定義し、b-ios（#779）は画面を変えない。
//!
//! # 状態機械
//!
//! ```text
//! Idle --公開--> StartRequested --mark--> Starting --finish_start-->
//!     Accepted    -> Waiting（OS の poweredOn・許可ダイアログ待ち。期限なし）
//!     Unsupported -> Idle（案内）
//!     Err         -> Idle（失敗）
//! Waiting --Advertising(session)--> Advertising
//! Waiting/Advertising --Ended(reason, session)--> Idle（理由別の文言）
//! Waiting/Advertising --停止--> StopRequested --mark--> Stopping --finish_stop--> Idle（停止しました）
//! ```
//!
//! 「公開中」は OS の開始コールバック成功の事象（[`ScreenEvent::Advertising`]）でだけ表示する。
//! 事象は `session`（公開を始めたときの世代）と一致し、`Waiting` / `Advertising` のときだけ反映する。
//! 読み取り回数は `Advertising` のときだけ足し、公開のたびに 0 へ戻す。停止処理中に届いた
//! 同じ session の終了は捨てる（停止の結果を正にする。資源は停止処理が解放する）。

use crate::{Screen, WidgetId};

/// 停止中の表示。
pub const IDLE_STATUS: &str = "停止中";
/// 準備中（OS の許可・電源状態の待ち）の表示。
pub const WAITING_STATUS: &str = "準備中…";
/// 公開中の表示。
pub const ADVERTISING_STATUS: &str = "公開中";
/// 処理中の表示。
pub const BUSY_STATUS: &str = "処理中…";
/// 利用者の停止で止まった（通常色）。
pub const STOPPED_NOTICE: &str = "停止しました";
/// iOS の背景遷移で止めた（通常色。失敗とは別の文言）。
pub const INTERRUPTED_NOTICE: &str = "アプリが背景へ移ったため止めました";
/// 非対応環境の案内（通常色）。
pub const UNSUPPORTED_NOTICE: &str = "この環境では使えません";
/// Bluetooth オフ。
pub const BLUETOOTH_OFF_ERROR: &str = "Bluetooth がオフです";
/// 権限拒否。
pub const PERMISSION_DENIED_ERROR: &str = "Bluetooth の許可がありません";
/// 公開の失敗。
pub const START_FAILED_ERROR: &str = "公開できませんでした";
/// 停止の失敗。
pub const STOP_FAILED_ERROR: &str = "停止できませんでした";

const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeripheralPhase {
    /// 停止中。
    #[default]
    Idle,
    /// 公開ボタンが押され、ドライバへまだ渡していない。
    StartRequested,
    /// ドライバが依頼を処理中。
    Starting,
    /// 依頼を受理済み。OS の準備（電源・許可）の待ち。
    Waiting,
    /// 公開中（開始のコールバックが成功した）。
    Advertising,
    /// 停止ボタンが押され、ドライバへまだ渡していない。
    StopRequested,
    /// ドライバが停止を処理中。
    Stopping,
}

/// 依頼の結果（非対応を加えたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartResult {
    /// 依頼を受理した（公開中ではない）。
    Accepted,
    /// この環境では非対応。
    Unsupported,
}

/// 停止の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopResult {
    /// 止めた。
    Stopped,
    /// 動いていなかった。
    NotRunning,
}

/// 失敗（画面には固定文言だけを出し、OS のコードは持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeripheralFailure;

/// 公開が終わった理由（画面側の区分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndKind {
    /// Bluetooth オフ。
    BluetoothOff,
    /// 権限拒否。
    PermissionDenied,
    /// 非対応。
    Unsupported,
    /// 失敗。
    Failed,
    /// 中断（iOS の背景遷移）。
    Interrupted,
}

/// OS から届いた事象（画面側の区分）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenEvent {
    /// 開始のコールバックが成功した。表示用のローカル名とサービス UUID はドライバが注入する。
    Advertising {
        /// アドバタイズ中のローカル名。
        local_name: String,
        /// アドバタイズ中のサービス UUID。
        service_uuid: String,
    },
    /// 公開が終わった。
    Ended(EndKind),
}

/// ドライバが行う操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeripheralAction {
    /// 公開を依頼する。
    Start,
    /// 停止する。
    Stop,
}

/// ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeripheralRequestView {
    /// 完了・事象へそのまま渡す世代番号。公開の要求では事象の session になる。
    pub generation: u64,
    /// 操作。
    pub action: PeripheralAction,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlePeripheralState {
    /// 進行状態。
    pub phase: PeripheralPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 現在の公開の session（公開を始めたときの世代）。
    pub session: u64,
    /// 今回の公開で読み取られた回数。
    pub reads: u64,
    /// 公開中のローカル名（`Advertising` のときだけ入る）。
    pub local_name: String,
    /// 公開中のサービス UUID（同上）。
    pub service_uuid: String,
    /// 停止・中断・非対応などの案内（通常色）。
    pub notice: String,
    /// 失敗系（`TextRole::Error`）。
    pub error: String,
    /// `Starting` 中に届いた事象の保留。受理結果（[`finish_start`]）の反映後に適用する。
    /// `Ended` は終端なので、保留したあとに届く事象は捨てる。
    pub pending_event: Option<ScreenEvent>,
}

fn clear_messages(state: &mut BlePeripheralState) {
    state.notice = String::new();
    state.error = String::new();
}

fn clear_advert_info(state: &mut BlePeripheralState) {
    state.local_name = String::new();
    state.service_uuid = String::new();
}

/// 「公開」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。公開のたびに回数を 0 へ戻す。
pub fn request_start(state: &mut BlePeripheralState) {
    if state.phase != PeripheralPhase::Idle {
        return;
    }
    state.phase = PeripheralPhase::StartRequested;
    state.generation = state.generation.wrapping_add(1);
    state.session = state.generation;
    state.reads = 0;
    state.pending_event = None;
    clear_advert_info(state);
    clear_messages(state);
}

/// 「停止」ボタンのハンドラ。`Waiting` / `Advertising` 以外は no-op。
pub fn request_stop(state: &mut BlePeripheralState) {
    if !matches!(
        state.phase,
        PeripheralPhase::Waiting | PeripheralPhase::Advertising
    ) {
        return;
    }
    state.phase = PeripheralPhase::StopRequested;
    state.generation = state.generation.wrapping_add(1);
    clear_messages(state);
}

/// ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_action(state: &BlePeripheralState) -> Option<PeripheralRequestView> {
    let action = match state.phase {
        PeripheralPhase::StartRequested => PeripheralAction::Start,
        PeripheralPhase::StopRequested => PeripheralAction::Stop,
        _ => return None,
    };
    Some(PeripheralRequestView {
        generation: state.generation,
        action,
    })
}

/// 要求を処理中へ進める。要求前のフェーズでは何もしない。
pub fn mark_in_flight(state: &mut BlePeripheralState) {
    state.phase = match state.phase {
        PeripheralPhase::StartRequested => PeripheralPhase::Starting,
        PeripheralPhase::StopRequested => PeripheralPhase::Stopping,
        other => other,
    };
}

/// 公開の依頼の結果を反映する。処理中でない・世代が違う完了は無視する。
/// 受理は「準備中」で、公開中ではない（公開中は [`apply_event`] だけが作る）。
pub fn finish_start(
    state: &mut BlePeripheralState,
    generation: u64,
    result: Result<StartResult, PeripheralFailure>,
) {
    if state.phase != PeripheralPhase::Starting || state.generation != generation {
        return;
    }
    clear_messages(state);
    let pending = state.pending_event.take();
    match result {
        Ok(StartResult::Accepted) => {
            state.phase = PeripheralPhase::Waiting;
            // 開始完了より先に届いた事象を、受理の反映後に現在の状態で評価し直す。
            if let Some(event) = pending {
                let session = state.session;
                apply_event(state, session, event);
            }
        }
        Ok(StartResult::Unsupported) => {
            state.phase = PeripheralPhase::Idle;
            state.notice = UNSUPPORTED_NOTICE.to_string();
        }
        Err(PeripheralFailure) => {
            state.phase = PeripheralPhase::Idle;
            state.error = START_FAILED_ERROR.to_string();
        }
    }
}

/// 停止の結果を反映する。処理中でない・世代が違う完了は無視する。
pub fn finish_stop(
    state: &mut BlePeripheralState,
    generation: u64,
    result: Result<StopResult, PeripheralFailure>,
) {
    if state.phase != PeripheralPhase::Stopping || state.generation != generation {
        return;
    }
    clear_messages(state);
    clear_advert_info(state);
    state.phase = PeripheralPhase::Idle;
    match result {
        Ok(StopResult::Stopped | StopResult::NotRunning) => {
            state.notice = STOPPED_NOTICE.to_string();
        }
        Err(PeripheralFailure) => state.error = STOP_FAILED_ERROR.to_string(),
    }
}

/// OS からの事象を反映する。`session` が現在の公開と一致し、`Waiting` / `Advertising` のときだけ
/// 反映する（古い世代・停止後・停止処理中の事象は捨てる）。`Starting` 中の事象は保留する。
pub fn apply_event(state: &mut BlePeripheralState, session: u64, event: ScreenEvent) {
    if state.session == session && state.phase == PeripheralPhase::Starting {
        // 開始結果より先に届いた事象は捨てず、finish_start の反映後に適用する。
        if !matches!(state.pending_event, Some(ScreenEvent::Ended(_))) {
            state.pending_event = Some(event);
        }
        return;
    }
    if state.session != session
        || !matches!(
            state.phase,
            PeripheralPhase::Waiting | PeripheralPhase::Advertising
        )
    {
        return;
    }
    match event {
        ScreenEvent::Advertising {
            local_name,
            service_uuid,
        } => {
            state.phase = PeripheralPhase::Advertising;
            state.local_name = local_name;
            state.service_uuid = service_uuid;
        }
        ScreenEvent::Ended(kind) => {
            clear_messages(state);
            clear_advert_info(state);
            state.phase = PeripheralPhase::Idle;
            match kind {
                EndKind::BluetoothOff => state.error = BLUETOOTH_OFF_ERROR.to_string(),
                EndKind::PermissionDenied => state.error = PERMISSION_DENIED_ERROR.to_string(),
                EndKind::Unsupported => state.notice = UNSUPPORTED_NOTICE.to_string(),
                EndKind::Failed => state.error = START_FAILED_ERROR.to_string(),
                EndKind::Interrupted => state.notice = INTERRUPTED_NOTICE.to_string(),
            }
        }
    }
}

/// 読み取り `n` 件を反映する。公開中（`Advertising`）かつ session が一致するときだけ数える。
pub fn apply_reads(state: &mut BlePeripheralState, session: u64, n: u64) {
    if state.phase == PeripheralPhase::Advertising && state.session == session {
        state.reads = state.reads.saturating_add(n);
    }
}

fn status_label(phase: PeripheralPhase) -> &'static str {
    match phase {
        PeripheralPhase::Idle => IDLE_STATUS,
        PeripheralPhase::Waiting => WAITING_STATUS,
        PeripheralPhase::Advertising => ADVERTISING_STATUS,
        _ => BUSY_STATUS,
    }
}

/// 状態の文言（`DynamicText` の `get`）。
pub fn state_text(state: &BlePeripheralState) -> String {
    status_label(state.phase).to_string()
}

/// 公開中の名前・UUID の文言（公開中以外は空）。
pub fn advert_text(state: &BlePeripheralState) -> String {
    if state.phase == PeripheralPhase::Advertising {
        format!("名前: {} / UUID: {}", state.local_name, state.service_uuid)
    } else {
        String::new()
    }
}

/// 読み取り回数の文言。
pub fn reads_text(state: &BlePeripheralState) -> String {
    format!("読み取られた回数: {}", state.reads)
}

/// 案内文言。
pub fn notice_text(state: &BlePeripheralState) -> String {
    state.notice.clone()
}

/// エラー文言。
pub fn error_text(state: &BlePeripheralState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。「公開」は `Idle` のときだけ、「停止」は `Waiting` / `Advertising` のときだけ出す。
pub fn current_screen(state: &BlePeripheralState) -> Screen<BlePeripheralState> {
    let screen = Screen::new("BLE の周辺機器役")
        .dynamic_text(state_text)
        .dynamic_text(advert_text)
        .dynamic_text(reads_text)
        .dynamic_text(notice_text)
        .error_text(error_text);
    match state.phase {
        PeripheralPhase::Idle => screen.button(START_BUTTON_ID, "公開", request_start),
        PeripheralPhase::Waiting | PeripheralPhase::Advertising => {
            screen.button(STOP_BUTTON_ID, "停止", request_stop)
        }
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starting() -> BlePeripheralState {
        let mut s = BlePeripheralState::default();
        request_start(&mut s);
        mark_in_flight(&mut s);
        s
    }

    fn waiting() -> BlePeripheralState {
        let mut s = starting();
        let g = s.generation;
        finish_start(&mut s, g, Ok(StartResult::Accepted));
        s
    }

    fn adv() -> ScreenEvent {
        ScreenEvent::Advertising {
            local_name: "N".to_string(),
            service_uuid: "U".to_string(),
        }
    }

    fn advertising() -> BlePeripheralState {
        let mut s = waiting();
        let sess = s.session;
        apply_event(&mut s, sess, adv());
        s
    }

    #[test]
    fn event_during_starting_is_held_until_accept() {
        let mut s = starting();
        let sess = s.session;
        apply_event(&mut s, sess, adv());
        apply_event(&mut s, sess, ScreenEvent::Ended(EndKind::BluetoothOff));
        // 終端の後に届いた事象は保留を上書きしない。
        apply_event(&mut s, sess, adv());
        assert_eq!(s.phase, PeripheralPhase::Starting);
        let g = s.generation;
        finish_start(&mut s, g, Ok(StartResult::Accepted));
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.error, BLUETOOTH_OFF_ERROR);
        assert!(s.pending_event.is_none());
    }

    #[test]
    fn pending_advertising_applies_after_accept_and_drops_on_failure() {
        let mut s = starting();
        let sess = s.session;
        apply_event(&mut s, sess, adv());
        let g = s.generation;
        finish_start(&mut s, g, Ok(StartResult::Accepted));
        assert_eq!(s.phase, PeripheralPhase::Advertising);

        let mut s = starting();
        let sess = s.session;
        apply_event(&mut s, sess, adv());
        let g = s.generation;
        finish_start(&mut s, g, Err(PeripheralFailure));
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert!(s.pending_event.is_none());
    }

    #[test]
    fn double_press_makes_one_request() {
        let mut s = BlePeripheralState::default();
        request_start(&mut s);
        let first = take_action(&s).unwrap();
        request_start(&mut s);
        assert_eq!(take_action(&s), Some(first));
        assert_eq!(s.generation, 1);
        mark_in_flight(&mut s);
        assert!(take_action(&s).is_none());
        request_start(&mut s);
        assert_eq!(s.phase, PeripheralPhase::Starting);
    }

    #[test]
    fn accepted_is_waiting_not_advertising() {
        let s = waiting();
        assert_eq!(s.phase, PeripheralPhase::Waiting);
        assert_eq!(state_text(&s), WAITING_STATUS);
    }

    #[test]
    fn advertising_only_on_event() {
        let s = advertising();
        assert_eq!(s.phase, PeripheralPhase::Advertising);
        assert_eq!(state_text(&s), ADVERTISING_STATUS);
        assert!(advert_text(&s).contains("UUID: U"));
    }

    #[test]
    fn unsupported_and_failure_never_advertise() {
        for (result, error, notice) in [
            (Ok(StartResult::Unsupported), "", UNSUPPORTED_NOTICE),
            (Err(PeripheralFailure), START_FAILED_ERROR, ""),
        ] {
            let mut s = starting();
            let g = s.generation;
            finish_start(&mut s, g, result);
            assert_eq!(s.phase, PeripheralPhase::Idle);
            assert_eq!(s.error, error);
            assert_eq!(s.notice, notice);
        }
    }

    #[test]
    fn end_reasons_have_distinct_messages() {
        let mut seen = Vec::new();
        for kind in [
            EndKind::BluetoothOff,
            EndKind::PermissionDenied,
            EndKind::Unsupported,
            EndKind::Failed,
            EndKind::Interrupted,
        ] {
            let mut s = advertising();
            let sess = s.session;
            apply_event(&mut s, sess, ScreenEvent::Ended(kind));
            assert_eq!(s.phase, PeripheralPhase::Idle);
            seen.push((s.error.clone(), s.notice.clone()));
        }
        // 失敗（START_FAILED_ERROR）以外は互いに異なる。中断は通常色で失敗の文言ではない。
        let mut uniq = seen.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), seen.len());
        assert_eq!(seen[4], (String::new(), INTERRUPTED_NOTICE.to_string()));
        assert_eq!(seen[1].0, PERMISSION_DENIED_ERROR);
    }

    #[test]
    fn stale_session_events_are_dropped() {
        let mut s = waiting();
        let stale = s.session + 1;
        apply_event(&mut s, stale, adv());
        assert_eq!(s.phase, PeripheralPhase::Waiting);
        apply_event(&mut s, stale, ScreenEvent::Ended(EndKind::Failed));
        assert_eq!(s.phase, PeripheralPhase::Waiting);
        let mut a = advertising();
        apply_reads(&mut a, stale, 3);
        assert_eq!(a.reads, 0);
    }

    #[test]
    fn events_after_stop_are_dropped() {
        let mut s = advertising();
        let sess = s.session;
        request_stop(&mut s);
        // 停止要求中・処理中の終了は捨てる（停止の結果を正にする）。
        apply_event(&mut s, sess, ScreenEvent::Ended(EndKind::Failed));
        assert_eq!(s.phase, PeripheralPhase::StopRequested);
        mark_in_flight(&mut s);
        apply_event(&mut s, sess, ScreenEvent::Ended(EndKind::Failed));
        assert_eq!(s.phase, PeripheralPhase::Stopping);
        apply_reads(&mut s, sess, 1);
        assert_eq!(s.reads, 0);
        let g = s.generation;
        finish_stop(&mut s, g, Ok(StopResult::Stopped));
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.notice, STOPPED_NOTICE);
        // 停止後の事象も捨てる。
        apply_event(&mut s, sess, adv());
        assert_eq!(s.phase, PeripheralPhase::Idle);
    }

    #[test]
    fn reads_count_only_while_advertising_and_reset_on_restart() {
        let mut w = waiting();
        let sess = w.session;
        apply_reads(&mut w, sess, 2);
        assert_eq!(w.reads, 0);

        let mut s = advertising();
        apply_reads(&mut s, sess, 2);
        apply_reads(&mut s, sess, 3);
        assert_eq!(s.reads, 5);
        assert_eq!(reads_text(&s), "読み取られた回数: 5");
        request_stop(&mut s);
        mark_in_flight(&mut s);
        let g = s.generation;
        finish_stop(&mut s, g, Ok(StopResult::NotRunning));
        request_start(&mut s);
        assert_eq!(s.reads, 0);
    }

    #[test]
    fn stale_and_unrequested_completions_are_ignored() {
        let mut s = starting();
        let g = s.generation + 1;
        finish_start(&mut s, g, Ok(StartResult::Accepted));
        assert_eq!(s.phase, PeripheralPhase::Starting);

        let mut idle = BlePeripheralState::default();
        finish_start(&mut idle, 0, Ok(StartResult::Accepted));
        finish_stop(&mut idle, 0, Ok(StopResult::Stopped));
        assert_eq!(idle.phase, PeripheralPhase::Idle);
        assert!(idle.notice.is_empty());
    }

    #[test]
    fn stop_failure_is_error_and_idle() {
        let mut s = advertising();
        request_stop(&mut s);
        mark_in_flight(&mut s);
        let g = s.generation;
        finish_stop(&mut s, g, Err(PeripheralFailure));
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.error, STOP_FAILED_ERROR);
    }

    #[test]
    fn stop_is_noop_unless_running() {
        let mut s = BlePeripheralState::default();
        request_stop(&mut s);
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.generation, 0);
    }

    #[test]
    fn screen_builds_in_every_phase() {
        for phase in [
            PeripheralPhase::Idle,
            PeripheralPhase::StartRequested,
            PeripheralPhase::Starting,
            PeripheralPhase::Waiting,
            PeripheralPhase::Advertising,
            PeripheralPhase::StopRequested,
            PeripheralPhase::Stopping,
        ] {
            let s = BlePeripheralState {
                phase,
                ..Default::default()
            };
            let _ = current_screen(&s);
        }
    }
}
