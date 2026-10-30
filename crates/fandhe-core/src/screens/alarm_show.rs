//! Phase 11（端末機能の拡充ループ、親 #140）型サンプル「アラーム・正確な時刻の
//! 予約」（Issue #721）。「1 分後にアラームを予約」「予約を取り消す」の 2 ボタンと、
//! 予約状態の表示を持つ 1 画面。`FANDHE_PROXY_APP=alarm_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod alarm_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。OS 呼び出しと現在時刻の取得は本
//! モジュール外（`fandhe-egui-proxy` の `device/alarm.rs`）が [`take_request`] で
//! 要求を観測し、[`finish_action`] で結果を注入する（`background_task.rs` と同型の
//! 状態機械）。`SystemTime` は wasm32 で panic するため本モジュールでは使わない。
//!
//! 予約状態は OS の予約一覧のポーリング結果（[`apply_snapshot`]）で更新する。
//! 取り消し操作なしに予約が一覧から消えた場合は「一覧から消えた」と表示し、
//! 「鳴った」とは言い切らない（`device-bridge-contracts.md`「6. 嘘の成功」。
//! 時刻が来たのか OS 側で取り消されたのかは区別できない）。
//!
//! ```text
//! Idle --ボタン--> Requested(Action), generation += 1
//!   --effect が take_request で観測 → mark_in_flight--> InFlight
//!   --finish_action（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::screens::background_task::format_utc;
use crate::{Screen, WidgetId};

/// 「予約」ボタンの `WidgetId`。
const SCHEDULE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「予約を取り消す」ボタンの `WidgetId`。
const CANCEL_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 予約ボタンのラベル。
pub const SCHEDULE_LABEL: &str = "1 分後にアラームを予約";
/// 取り消しボタンのラベル。
pub const CANCEL_LABEL: &str = "予約を取り消す";
/// 予約に成功したときの案内。「鳴った」とは言わない。
pub const SCHEDULED_STATUS: &str =
    "1 分後にアラームを予約しました（時刻になると OS がアラームで知らせます）";
/// 取り消しに成功したときの案内（未予約での取り消しも契約上 `Ok`）。
pub const CANCELLED_STATUS: &str = "予約を取り消しました";
/// 取り消し操作なしに予約が一覧から消えたときの案内。
pub const CLEARED_STATUS: &str =
    "予約の一覧から消えました（時刻が来たか、OS 側で取り消されました）";
/// 権限拒否（iOS の許可ダイアログでの拒否・拒否済み、Android の正確なアラームの許可なし・通知無効）の固定文言。
pub const PERMISSION_DENIED_TEXT: &str =
    "アラームの許可がありません。設定で許可してからもう一度押してください";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 予約一覧の取得に失敗したときの併記文言（OS のエラー文は出さない）。
pub const CHECK_FAILED_TEXT: &str = "予約状態を確認できませんでした";
/// 画面に常時出す補足。
pub const NOTE: &str = "通知やアラームの許可が無いと予約できません。iOS は 26 以上が対象です。時刻は UTC で表示します。";

/// ボタン操作の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 「1 分後にアラームを予約」。
    Schedule,
    /// 「予約を取り消す」。
    Cancel,
}

/// ボタン操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested(Action),
    /// effect ドライバが OS 呼び出しを処理中（[`mark_in_flight`] 済み）。
    InFlight,
}

/// 予約状態の表示用写像（`fandhe_device::alarm::AlarmState` と対応する。
/// `fandhe-core` は `fandhe-device` に依存しないため、写像は `fandhe-egui-proxy`
/// 側の責務にする）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScheduleView {
    /// まだ一度も観測していない。
    #[default]
    Unknown,
    /// 予約が一覧に無い。
    NotScheduled,
    /// 予約が一覧に残っている。
    Scheduled {
        /// 鳴る時刻（UNIX エポックからのミリ秒）。
        fire_at_unix_ms: u64,
    },
}

/// アラーム画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlarmShowState {
    /// ボタン操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// OS の予約一覧の観測結果。取得に失敗しても直前の確定値は消さない。
    pub schedule_view: ScheduleView,
    /// 直近の予約一覧の取得が失敗したか。`schedule_view` は「最後に確認できた状態」
    /// のままで、画面には確認失敗を併記する（[`apply_check_failure`]）。
    pub check_failed: bool,
    /// 成功・取り消し・一覧から消えた旨の案内（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列。
    pub error: String,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// [`finish_action`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 実行する操作。
    pub action: Action,
}

/// 「予約」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op。
pub fn schedule(state: &mut AlarmShowState) {
    if state.phase != Phase::Idle {
        return;
    }
    request(state, Action::Schedule);
}

/// 「予約を取り消す」ボタンのハンドラ。未予約でも要求を出す（契約上 `Ok`）。
pub fn cancel(state: &mut AlarmShowState) {
    if state.phase != Phase::Idle {
        return;
    }
    request(state, Action::Cancel);
}

fn request(state: &mut AlarmShowState, action: Action) {
    state.phase = Phase::Requested(action);
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
    state.check_failed = false;
}

/// effect ドライバが未発送の要求を観測する getter（状態は変えない）。
pub fn take_request(state: &AlarmShowState) -> Option<Request> {
    match state.phase {
        Phase::Requested(action) => Some(Request {
            generation: state.generation,
            action,
        }),
        _ => None,
    }
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut AlarmShowState) {
    if matches!(state.phase, Phase::Requested(_)) {
        state.phase = Phase::InFlight;
    }
}

/// 操作結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は
/// 無視する（古い世代の完了を成功にしない）。予約成功時の `fire_at_unix_ms` は
/// 実際に OS へ渡した時刻で、表示用に `schedule_view` へ入れる。
pub fn finish_action(
    state: &mut AlarmShowState,
    generation: u64,
    action: Action,
    result: Result<Option<u64>, String>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    match result {
        Ok(fire_at) => {
            state.error = String::new();
            match action {
                Action::Schedule => {
                    state.status = SCHEDULED_STATUS.to_string();
                    if let Some(fire_at_unix_ms) = fire_at {
                        state.schedule_view = ScheduleView::Scheduled { fire_at_unix_ms };
                    }
                }
                Action::Cancel => {
                    // 取り消しは成功として出す（失敗として見せない）。
                    state.status = CANCELLED_STATUS.to_string();
                    state.schedule_view = ScheduleView::NotScheduled;
                }
            }
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// OS の予約一覧のポーリング結果を反映する（`phase` には触れない）。
///
/// `generation` は読み取りを始めた時点の [`AlarmShowState::generation`]。現在の世代と
/// 違う（読んでいる間に予約・取り消しの要求が立った）、または `Idle` 以外（操作中で、
/// 結果が [`finish_action`] で確定する）のときは古い観測として捨てる。操作中でない
/// ときに `Scheduled` から `NotScheduled` へ変わったら、取り消し操作なしに一覧から
/// 消えたとして案内を出す。
pub fn apply_snapshot(state: &mut AlarmShowState, view: ScheduleView, generation: u64) {
    if state.phase != Phase::Idle || state.generation != generation {
        return;
    }
    let was_scheduled = matches!(state.schedule_view, ScheduleView::Scheduled { .. });
    if was_scheduled && view == ScheduleView::NotScheduled {
        // `error` は直前の操作の失敗理由なので、ポーリングでは消さない
        // （消すのは次の利用者操作の開始と操作の成功完了だけ）。
        state.status = CLEARED_STATUS.to_string();
    }
    state.schedule_view = view;
    state.check_failed = false;
}

/// 予約一覧の取得失敗を反映する。`schedule_view`（最後に確認できた状態）と
/// `status` は消さず、確認失敗の印だけを立てる。世代・`Idle` の照合は
/// [`apply_snapshot`] と同じで、古い観測は捨てる。
pub fn apply_check_failure(state: &mut AlarmShowState, generation: u64) {
    if state.phase != Phase::Idle || state.generation != generation {
        return;
    }
    state.check_failed = true;
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &AlarmShowState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &AlarmShowState) -> String {
    state.error.clone()
}

/// 予約状態の案内文言（`DynamicText` の `get`）。
pub fn schedule_text(state: &AlarmShowState) -> String {
    let base = match state.schedule_view {
        ScheduleView::Unknown => "予約状態: ...".to_string(),
        ScheduleView::NotScheduled => "予約状態: 未予約".to_string(),
        ScheduleView::Scheduled { fire_at_unix_ms } => {
            format!("予約状態: 予約中（{}）", format_utc(fire_at_unix_ms))
        }
    };
    if !state.check_failed {
        return base;
    }
    match state.schedule_view {
        // 一度も確認できていない場合は「...」のままにせず失敗を言い切る。
        ScheduleView::Unknown => format!("予約状態: {CHECK_FAILED_TEXT}"),
        _ => format!("{base}（最後に確認できた状態。{CHECK_FAILED_TEXT}）"),
    }
}

/// アラーム画面を組み立てる。`Idle` のときだけボタンを出し、処理中は二重押しを
/// 防ぐため外す。
pub fn current_screen(state: &AlarmShowState) -> Screen<AlarmShowState> {
    let screen = Screen::new("アラーム")
        .text(NOTE)
        .dynamic_text(schedule_text)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != Phase::Idle {
        return screen;
    }
    screen
        .button(SCHEDULE_BUTTON_ID, SCHEDULE_LABEL, schedule)
        .button(CANCEL_BUTTON_ID, CANCEL_LABEL, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn in_flight(action: Action) -> AlarmShowState {
        let mut s = AlarmShowState::default();
        match action {
            Action::Schedule => schedule(&mut s),
            Action::Cancel => cancel(&mut s),
        }
        mark_in_flight(&mut s);
        s
    }

    #[test]
    fn schedule_advances_generation_and_phase() {
        let mut s = AlarmShowState::default();
        schedule(&mut s);
        assert_eq!(s.phase, Phase::Requested(Action::Schedule));
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn requests_are_not_queued_while_busy() {
        let mut s = in_flight(Action::Schedule);
        schedule(&mut s);
        cancel(&mut s);
        assert_eq!(s.phase, Phase::InFlight);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn take_request_only_when_requested() {
        let mut s = AlarmShowState::default();
        assert_eq!(take_request(&s), None);
        schedule(&mut s);
        assert_eq!(
            take_request(&s),
            Some(Request {
                generation: 1,
                action: Action::Schedule
            })
        );
        mark_in_flight(&mut s);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn schedule_success_shows_scheduled_without_claiming_it_rang() {
        let mut s = in_flight(Action::Schedule);
        finish_action(&mut s, 1, Action::Schedule, Ok(Some(1_700_000_060_000)));
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(s.status, SCHEDULED_STATUS);
        assert_eq!(
            s.schedule_view,
            ScheduleView::Scheduled {
                fire_at_unix_ms: 1_700_000_060_000
            }
        );
        assert!(!s.status.contains("鳴りました"));
        assert!(schedule_text(&s).contains("UTC"));
    }

    #[test]
    fn cancel_success_is_not_shown_as_failure() {
        let mut s = in_flight(Action::Cancel);
        finish_action(&mut s, 1, Action::Cancel, Ok(None));
        assert_eq!(s.status, CANCELLED_STATUS);
        assert_eq!(s.error, "");
        assert_eq!(s.schedule_view, ScheduleView::NotScheduled);
    }

    #[test]
    fn error_is_shown_and_never_as_scheduled() {
        let mut s = in_flight(Action::Schedule);
        finish_action(
            &mut s,
            1,
            Action::Schedule,
            Err("権限が拒否されました: notification".to_string()),
        );
        assert_eq!(s.status, "");
        assert!(s.error.starts_with(FAILED_PREFIX));
        assert!(!matches!(s.schedule_view, ScheduleView::Scheduled { .. }));
    }

    #[test]
    fn stale_generation_completion_changes_nothing() {
        let mut s = in_flight(Action::Schedule);
        // 世代 1 の完了で Idle に戻した後、取り消しを要求（世代 2）。
        finish_action(&mut s, 1, Action::Schedule, Ok(Some(1)));
        cancel(&mut s);
        let before = s.clone();
        finish_action(&mut s, 1, Action::Schedule, Ok(Some(2)));
        assert_eq!(s, before);
    }

    #[test]
    fn completion_when_idle_is_ignored() {
        let mut s = AlarmShowState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_action(&mut s, 0, Action::Schedule, Ok(Some(1)));
        assert_eq!(s.status, "変わらない");
        assert_eq!(s.schedule_view, ScheduleView::Unknown);
    }

    #[test]
    fn snapshot_scheduled_to_not_scheduled_reports_cleared() {
        let mut s = AlarmShowState::default();
        apply_snapshot(&mut s, ScheduleView::Scheduled { fire_at_unix_ms: 5 }, 0);
        apply_snapshot(&mut s, ScheduleView::NotScheduled, 0);
        assert_eq!(s.status, CLEARED_STATUS);
        assert!(!s.status.contains("鳴りました"));
    }

    #[test]
    fn snapshot_not_scheduled_keeps_previous_action_error() {
        let mut s = AlarmShowState::default();
        schedule(&mut s);
        mark_in_flight(&mut s);
        finish_action(&mut s, 1, Action::Schedule, Ok(Some(5)));
        // 再予約: 古い予約の削除後に追加が失敗し、表示は Scheduled のまま残る。
        schedule(&mut s);
        mark_in_flight(&mut s);
        finish_action(&mut s, 2, Action::Schedule, Err("失敗".to_string()));
        assert!(s.error.starts_with(FAILED_PREFIX));
        apply_snapshot(&mut s, ScheduleView::NotScheduled, 2);
        assert!(s.error.starts_with(FAILED_PREFIX));
        assert_eq!(s.schedule_view, ScheduleView::NotScheduled);
    }

    #[test]
    fn snapshot_while_busy_is_discarded() {
        let mut s = AlarmShowState {
            schedule_view: ScheduleView::Scheduled { fire_at_unix_ms: 5 },
            ..Default::default()
        };
        schedule(&mut s);
        let generation = s.generation;
        apply_snapshot(&mut s, ScheduleView::NotScheduled, generation);
        assert_eq!(s.status, "");
        assert_eq!(
            s.schedule_view,
            ScheduleView::Scheduled { fire_at_unix_ms: 5 }
        );
        mark_in_flight(&mut s);
        apply_snapshot(&mut s, ScheduleView::NotScheduled, generation);
        assert_eq!(
            s.schedule_view,
            ScheduleView::Scheduled { fire_at_unix_ms: 5 }
        );
        assert_eq!(s.phase, Phase::InFlight);
    }

    #[test]
    fn snapshot_read_before_a_newer_action_is_discarded() {
        // 世代 0 で読み始めたポーリングが、予約の完了（世代 1・Idle）より後に届く。
        let mut s = AlarmShowState::default();
        schedule(&mut s);
        mark_in_flight(&mut s);
        finish_action(&mut s, 1, Action::Schedule, Ok(Some(9)));
        apply_snapshot(&mut s, ScheduleView::NotScheduled, 0);
        assert_eq!(
            s.schedule_view,
            ScheduleView::Scheduled { fire_at_unix_ms: 9 }
        );
        assert_eq!(s.status, SCHEDULED_STATUS);
    }

    #[test]
    fn check_failure_keeps_last_known_scheduled_and_notes_failure() {
        let mut s = AlarmShowState::default();
        schedule(&mut s);
        mark_in_flight(&mut s);
        finish_action(&mut s, 1, Action::Schedule, Ok(Some(9)));
        apply_check_failure(&mut s, 1);
        assert_eq!(
            s.schedule_view,
            ScheduleView::Scheduled { fire_at_unix_ms: 9 }
        );
        assert_eq!(s.status, SCHEDULED_STATUS);
        assert!(schedule_text(&s).contains("予約中"));
        assert!(schedule_text(&s).contains(CHECK_FAILED_TEXT));
    }

    #[test]
    fn check_failure_before_any_observation_is_stated() {
        let mut s = AlarmShowState::default();
        apply_check_failure(&mut s, 0);
        assert_eq!(schedule_text(&s), format!("予約状態: {CHECK_FAILED_TEXT}"));
    }

    #[test]
    fn snapshot_after_failure_recovers_and_clears_mark() {
        let mut s = AlarmShowState::default();
        apply_snapshot(&mut s, ScheduleView::Scheduled { fire_at_unix_ms: 5 }, 0);
        apply_check_failure(&mut s, 0);
        apply_snapshot(&mut s, ScheduleView::NotScheduled, 0);
        assert!(!s.check_failed);
        assert_eq!(s.status, CLEARED_STATUS);
        assert_eq!(schedule_text(&s), "予約状態: 未予約");
    }

    #[test]
    fn stale_or_busy_check_failure_is_discarded() {
        let mut s = AlarmShowState::default();
        schedule(&mut s);
        apply_check_failure(&mut s, 1); // Requested 中
        assert!(!s.check_failed);
        mark_in_flight(&mut s);
        finish_action(&mut s, 1, Action::Schedule, Ok(Some(9)));
        apply_check_failure(&mut s, 0); // 古い世代
        assert!(!s.check_failed);
    }

    #[test]
    fn new_request_clears_check_failure_mark() {
        let mut s = AlarmShowState::default();
        apply_check_failure(&mut s, 0);
        schedule(&mut s);
        assert!(!s.check_failed);
    }

    #[test]
    fn screen_shows_labels_and_hides_buttons_while_busy() {
        let mut s = AlarmShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, SCHEDULE_LABEL, &mut s));
        let mut busy = in_flight(Action::Schedule);
        let screen = current_screen(&busy);
        assert!(!click_button(&screen, SCHEDULE_LABEL, &mut busy));
        assert!(!click_button(&screen, CANCEL_LABEL, &mut busy));
    }
}
