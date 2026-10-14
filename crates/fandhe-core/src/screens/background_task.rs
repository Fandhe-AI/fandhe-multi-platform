//! Phase 11（端末機能の拡充ループ、親 #1042）型サンプル「バックグラウンド処理」
//! （Issue #1043）。「予約する」「予約を取り消す」の 2 ボタン + 予約状態・最後の
//! 実行結果の表示を持つ 1 画面。`FANDHE_PROXY_APP=background-task` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod background_task;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の OS 呼び出しは本モジュール外
//! （`fandhe-egui-proxy` の effect ドライバ、`device/background_task.rs`）が
//! [`take_request`] で要求を観測し [`finish_action`] で結果を注入する
//! （`notification_send.rs` と同型の状態機械）。
//!
//! 他の型サンプルと異なり、OS が利用者の操作なしに非同期で処理を実行しうる
//! （予約 → バックグラウンドで発火 → 保存）ため、予約状態・最後の実行結果は
//! ボタン操作の結果だけでなく effect ドライバによる定期的な観測（ポーリング）でも
//! 更新される（[`apply_snapshot`]）。「予約しました」の文言は「実行されるタイミングは
//! OS 次第です」までに留め、「実行しました」とは言い切らない
//! （`device-bridge-contracts.md`「6. 嘘の成功」「10. キャンセル・正常終了・失敗の
//! 三値」参照）。
//!
//! ```text
//! Idle --「予約する」/「予約を取り消す」--> phase=Requested(Action), generation += 1
//!   --effect が take_request で観測 → mark_in_flight--> phase=InFlight
//!   --finish_action（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```
//!
//! 並行して、effect ドライバはポーリングで [`apply_snapshot`] を呼び、OS 側の
//! 予約状態・最後の実行結果を画面へ反映する（ボタン操作とは独立。`phase` には
//! 触れない）。

use crate::{Screen, WidgetId};

/// 「予約する」ボタンの `WidgetId`。
const SCHEDULE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「予約を取り消す」ボタンの `WidgetId`。
const CANCEL_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 予約に成功したときの案内。「実行しました」とは言い切らない
/// （`device-bridge-contracts.md`「6. 嘘の成功」「10. ...」参照）。
pub const SCHEDULED_STATUS: &str = "予約しました（実行されるタイミングは OS 次第です）";
/// 取り消しに成功したときの案内（未予約での取り消しも契約上 `Ok`）。
pub const CANCELLED_STATUS: &str = "予約を取り消しました";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// ボタン操作の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 「予約する」。
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

/// 予約状態の表示用写像（`fandhe_device::background_task::ScheduleState` と対応する。
/// `fandhe-core` は `fandhe-device` に依存しないため、写像は `fandhe-egui-proxy` 側の
/// 責務にする、モジュール doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScheduleView {
    /// まだ一度も観測していない（画面を開いた直後）。
    #[default]
    Unknown,
    /// 予約していない。
    NotScheduled,
    /// 予約済み（実行時刻は保証しない）。
    Scheduled,
    /// 予約が実行済み。
    Ran,
}

/// 実行結果の表示用写像。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResultView {
    /// 処理が完了した。
    Completed,
    /// OS の実行可能時間切れで中断された。
    Expired,
}

/// 最後の実行結果の表示用写像。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LastRunView {
    /// まだ一度も観測していない。
    #[default]
    Unloaded,
    /// 未実行。
    NeverRan,
    /// 実行済み（時刻は UNIX エポックからのミリ秒、UTC 表示は [`format_utc`]）。
    Ran { at_ms: u64, result: RunResultView },
    /// 保存結果の読み出しに失敗した（未実行と区別して表示する、
    /// `device-bridge-contracts.md`「6. 嘘の成功」参照）。読み出し失敗の文言
    /// （`fandhe_device::DeviceError` の `Display`）をそのまま保持する。
    ReadFailed(String),
}

/// バックグラウンド処理画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackgroundTaskState {
    /// ボタン操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号。要求のたびに進み、完了通知が「今の要求」を
    /// 指しているかの照合に使う（モジュール doc 参照）。
    pub generation: u64,
    /// OS 側の予約状態（ポーリングで更新、[`apply_snapshot`]）。
    pub schedule_view: ScheduleView,
    /// `schedule_view` が最後に [`ScheduleView::Ran`] へ更新されたときの
    /// `generation`（[`apply_snapshot`] が更新）。[`finish_action`] が
    /// 「発火が完了通知より先に届いた（今の要求と同じ世代の `Ran`）」のか
    /// 「前回までの実行が残っているだけ（古い世代の `Ran`）」のかを区別するために
    /// 持つ。`Ran` 以外へ遷移したら `None` に戻す（Bugbot 指摘、PR #1048
    /// レビュースレッド参照: 新しい `schedule` 成功時に古い世代の `Ran` を
    /// 「発火済みだから」と誤って保持し続け、予約状態の表示が実行済みのまま
    /// 固まる不具合の修正）。
    pub ran_at_generation: Option<u64>,
    /// OS 側の最後の実行結果（ポーリングで更新、[`apply_snapshot`]）。
    pub last_run_view: LastRunView,
    /// 成功・キャンセルの案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列にする。
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

/// 「予約する」ボタンのハンドラ。`Idle` 以外、または既に `Scheduled` のときは
/// 二重要求を防ぐため no-op（panic を API 契約にしない、coding-rust.md）。
pub fn schedule(state: &mut BackgroundTaskState) {
    if state.phase != Phase::Idle || state.schedule_view == ScheduleView::Scheduled {
        return;
    }
    request(state, Action::Schedule);
}

/// 「予約を取り消す」ボタンのハンドラ。`Idle` のときは常に出す（未予約での取り消しも
/// 契約上 `Ok`、モジュール doc 参照）。二重要求だけ防ぐ。
pub fn cancel(state: &mut BackgroundTaskState) {
    if state.phase != Phase::Idle {
        return;
    }
    request(state, Action::Cancel);
}

fn request(state: &mut BackgroundTaskState, action: Action) {
    state.phase = Phase::Requested(action);
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果表示（status/error）を残さない（`notification_send::notify` と同じ判断）。
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない。
/// 実際に `InFlight` へ進めるのは [`mark_in_flight`]）。
pub fn take_request(state: &BackgroundTaskState) -> Option<Request> {
    match state.phase {
        Phase::Requested(action) => Some(Request {
            generation: state.generation,
            action,
        }),
        _ => None,
    }
}

/// 要求を処理中（`InFlight`）へ進める。`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut BackgroundTaskState) {
    if matches!(state.phase, Phase::Requested(_)) {
        state.phase = Phase::InFlight;
    }
}

/// 操作結果を反映する。`generation` は [`take_request`] が返した世代番号
/// （呼び出し元が処理中も保持し続ける契約）。現在の世代と一致しない、または
/// `Idle`（要求が無い）で届いた完了は無視する（`notification_send::finish_notify`
/// と同型）。成功時は `action` に応じた文言・`schedule_view` を設定する。
pub fn finish_action(
    state: &mut BackgroundTaskState,
    generation: u64,
    action: Action,
    result: Result<(), String>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    match result {
        Ok(()) => {
            state.error = String::new();
            match action {
                Action::Schedule => {
                    state.status = SCHEDULED_STATUS.to_string();
                    // 発火済み（`Ran`、ポーリングの `apply_snapshot` が既に観測済み）を
                    // 完了通知で「予約中」へ巻き戻さない。OS 呼び出しの成功は「予約を
                    // 受理した」ことしか意味せず、発火が完了通知より先に UI へ届いた
                    // 場合、ここで無条件に上書きすると次の再描画まで嘘の「予約中」が
                    // 残る（`device-bridge-contracts.md`「4. ポーリング・再描画」
                    // 「6. 嘘の成功」参照）。ただし保持してよいのは「今の要求と同じ
                    // 世代で観測された `Ran`」だけ。前回までの実行が残っているだけの
                    // 古い世代の `Ran` まで保持すると、再予約が成功しても画面が
                    // 「実行済み」のまま固まる（Bugbot 指摘、PR #1048）。
                    let ran_this_generation = state.schedule_view == ScheduleView::Ran
                        && state.ran_at_generation == Some(generation);
                    if !ran_this_generation {
                        state.schedule_view = ScheduleView::Scheduled;
                    }
                }
                Action::Cancel => {
                    state.status = CANCELLED_STATUS.to_string();
                    state.schedule_view = ScheduleView::NotScheduled;
                    state.ran_at_generation = None;
                }
            }
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// OS 側の予約状態・最後の実行結果をポーリング結果で反映する（`phase` には触れない、
/// モジュール doc 参照）。effect ドライバは差分が無ければ呼ばない契約だが、本関数
/// 自体は冪等（同じ値を渡しても状態は変わらない）。
pub fn apply_snapshot(
    state: &mut BackgroundTaskState,
    schedule_view: ScheduleView,
    last_run_view: LastRunView,
) {
    // `Ran` を観測した世代を記録する（[`finish_action`] が「今の要求の発火」と
    // 「前回までの実行が残っているだけ」を区別するために使う、`ran_at_generation`
    // のフィールド doc 参照）。`Ran` 以外へ遷移したら忘れる。
    state.ran_at_generation = if schedule_view == ScheduleView::Ran {
        Some(state.generation)
    } else {
        None
    };
    state.schedule_view = schedule_view;
    state.last_run_view = last_run_view;
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &BackgroundTaskState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`error_text` の `get`。`TextRole::Error` として描画される）。
pub fn error_text(state: &BackgroundTaskState) -> String {
    state.error.clone()
}

/// 1970-01-01 からの経過日数 → `(year, month, day)` へ変換する純関数（Howard
/// Hinnant の civil_from_days アルゴリズム）。時刻ライブラリは追加しない方針
/// （`ponytail:` 現地時刻が必要になったら tz 対応を足す、本画面は UTC 固定で足りる）。
/// Issue #1044: Windows 実装（`windows_background_task.rs::start_boundary`）が
/// `ITimeTrigger::StartBoundary` の `YYYY-MM-DDTHH:MM:SS+00:00` 整形にも使う。
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `ran_at_unix_ms`（UNIX エポックからのミリ秒）を `YYYY-MM-DD HH:MM:SS UTC` へ
/// 整形する純関数（[`LastRunView::Ran`] の表示用）。
pub fn format_utc(ran_at_unix_ms: u64) -> String {
    let total_secs = ran_at_unix_ms / 1000;
    let days = (total_secs / 86_400) as i64;
    let secs_of_day = total_secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let h = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}:{s:02} UTC")
}

fn result_label(result: RunResultView) -> &'static str {
    match result {
        RunResultView::Completed => "完了",
        RunResultView::Expired => "時間切れ",
    }
}

/// 最後の実行結果の案内文言（`DynamicText` の `get`）。
pub fn last_run_text(state: &BackgroundTaskState) -> String {
    match &state.last_run_view {
        LastRunView::Unloaded => "最後の実行: ...".to_string(),
        LastRunView::NeverRan => "最後の実行: 未実行".to_string(),
        LastRunView::Ran { at_ms, result } => {
            format!(
                "最後の実行: {}（{}）",
                format_utc(*at_ms),
                result_label(*result)
            )
        }
        LastRunView::ReadFailed(message) => {
            format!("最後の実行の読み出しに失敗しました: {message}")
        }
    }
}

/// 予約状態の案内文言（`DynamicText` の `get`）。
pub fn schedule_text(state: &BackgroundTaskState) -> String {
    match state.schedule_view {
        ScheduleView::Unknown => "予約状態: ...".to_string(),
        ScheduleView::NotScheduled => "予約状態: 未予約".to_string(),
        ScheduleView::Scheduled => "予約状態: 予約中".to_string(),
        ScheduleView::Ran => "予約状態: 実行済み".to_string(),
    }
}

/// バックグラウンド処理画面を組み立てる。`registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。`Idle` のときのみボタンを出し、
/// 処理中（`Requested`/`InFlight`）は二重押しを防ぐため外す
/// （`notification_send::current_screen` と同型）。
pub fn current_screen(state: &BackgroundTaskState) -> Screen<BackgroundTaskState> {
    let screen = Screen::new("バックグラウンド処理")
        .dynamic_text(schedule_text)
        .dynamic_text(last_run_text)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != Phase::Idle {
        return screen;
    }
    let screen = if state.schedule_view != ScheduleView::Scheduled {
        screen.button(SCHEDULE_BUTTON_ID, "予約する", schedule)
    } else {
        screen
    };
    screen.button(CANCEL_BUTTON_ID, "予約を取り消す", cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_advances_generation_and_phase() {
        let mut state = BackgroundTaskState::default();
        schedule(&mut state);
        assert_eq!(state.phase, Phase::Requested(Action::Schedule));
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn schedule_is_noop_while_already_scheduled() {
        let mut state = BackgroundTaskState {
            schedule_view: ScheduleView::Scheduled,
            ..Default::default()
        };
        schedule(&mut state);
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.generation, 0);
    }

    #[test]
    fn schedule_is_noop_while_in_flight() {
        let mut state = BackgroundTaskState {
            phase: Phase::InFlight,
            generation: 1,
            ..Default::default()
        };
        schedule(&mut state);
        assert_eq!(state.phase, Phase::InFlight);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn cancel_advances_generation_even_when_not_scheduled() {
        // 未予約での取り消しも契約上 Ok（モジュール doc 参照）。画面側は要求を
        // 立てることだけ担い、Ok/Err の判定は effect ドライバ側で行う。
        let mut state = BackgroundTaskState {
            schedule_view: ScheduleView::NotScheduled,
            ..Default::default()
        };
        cancel(&mut state);
        assert_eq!(state.phase, Phase::Requested(Action::Cancel));
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn cancel_is_noop_while_in_flight() {
        let mut state = BackgroundTaskState {
            phase: Phase::InFlight,
            ..Default::default()
        };
        cancel(&mut state);
        assert_eq!(state.phase, Phase::InFlight);
        assert_eq!(state.generation, 0);
    }

    #[test]
    fn take_request_returns_generation_and_action_only_when_requested() {
        let mut state = BackgroundTaskState::default();
        assert_eq!(take_request(&state), None);
        schedule(&mut state);
        assert_eq!(
            take_request(&state),
            Some(Request {
                generation: 1,
                action: Action::Schedule,
            })
        );
        mark_in_flight(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn finish_action_schedule_success_sets_scheduled_view() {
        let mut state = BackgroundTaskState::default();
        schedule(&mut state);
        mark_in_flight(&mut state);
        finish_action(&mut state, 1, Action::Schedule, Ok(()));
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.schedule_view, ScheduleView::Scheduled);
        assert_eq!(state.status, SCHEDULED_STATUS);
        assert!(!state.status.contains("実行しました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_action_schedule_success_does_not_revert_already_fired_view() {
        // 発火（`on_fired` → ポーリングの `apply_snapshot`）が完了通知より先に届いた
        // 場合、完了通知で `Ran` を「予約中」へ戻さない（モジュール doc 参照）。
        let mut state = BackgroundTaskState::default();
        schedule(&mut state);
        mark_in_flight(&mut state);
        apply_snapshot(&mut state, ScheduleView::Ran, LastRunView::NeverRan);
        finish_action(&mut state, 1, Action::Schedule, Ok(()));
        assert_eq!(state.schedule_view, ScheduleView::Ran);
        assert_eq!(state.status, SCHEDULED_STATUS);
    }

    #[test]
    fn finish_action_reschedule_success_clears_stale_ran_view() {
        // 前回の実行で `Ran` になった後、新しい `schedule` が成功した場合は
        // 「前回までの実行が残っているだけ」の古い世代の `Ran` であり、今回の
        // 要求の発火ではない。`Scheduled` へ正しく進む必要がある
        // （Bugbot 指摘「Reschedule keeps stale Ran view」、PR #1048）。
        let mut state = BackgroundTaskState::default();
        schedule(&mut state); // generation = 1
        mark_in_flight(&mut state);
        finish_action(&mut state, 1, Action::Schedule, Ok(()));
        apply_snapshot(&mut state, ScheduleView::Ran, LastRunView::NeverRan); // 前回の発火

        schedule(&mut state); // generation = 2（再予約）
        mark_in_flight(&mut state);
        finish_action(&mut state, 2, Action::Schedule, Ok(()));
        assert_eq!(state.schedule_view, ScheduleView::Scheduled);
        assert_eq!(state.status, SCHEDULED_STATUS);
    }

    #[test]
    fn finish_action_cancel_success_sets_not_scheduled_view() {
        let mut state = BackgroundTaskState {
            schedule_view: ScheduleView::Scheduled,
            ..Default::default()
        };
        cancel(&mut state);
        mark_in_flight(&mut state);
        finish_action(&mut state, 1, Action::Cancel, Ok(()));
        assert_eq!(state.schedule_view, ScheduleView::NotScheduled);
        assert_eq!(state.status, CANCELLED_STATUS);
    }

    #[test]
    fn finish_action_error_sets_error_without_success_status() {
        let mut state = BackgroundTaskState::default();
        schedule(&mut state);
        mark_in_flight(&mut state);
        finish_action(
            &mut state,
            1,
            Action::Schedule,
            Err("この環境ではバックグラウンド処理を使えません".to_string()),
        );
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn finish_action_ignores_stale_generation() {
        let mut state = BackgroundTaskState::default();
        schedule(&mut state); // generation = 1
        mark_in_flight(&mut state);
        finish_action(&mut state, 1, Action::Schedule, Ok(())); // Idle に戻る

        cancel(&mut state); // generation = 2
        finish_action(&mut state, 1, Action::Schedule, Ok(()));
        // 古い世代の完了は無視され、Requested(Cancel) のまま
        assert_eq!(state.phase, Phase::Requested(Action::Cancel));
    }

    #[test]
    fn finish_action_ignores_completion_when_idle() {
        let mut state = BackgroundTaskState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_action(&mut state, 0, Action::Schedule, Ok(()));
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn apply_snapshot_does_not_touch_phase() {
        let mut state = BackgroundTaskState {
            phase: Phase::InFlight,
            ..Default::default()
        };
        apply_snapshot(&mut state, ScheduleView::Ran, LastRunView::NeverRan);
        assert_eq!(state.phase, Phase::InFlight);
        assert_eq!(state.schedule_view, ScheduleView::Ran);
    }

    #[test]
    fn last_run_text_distinguishes_read_failed_from_never_ran() {
        let mut never = BackgroundTaskState::default();
        apply_snapshot(
            &mut never,
            ScheduleView::NotScheduled,
            LastRunView::NeverRan,
        );
        assert_eq!(last_run_text(&never), "最後の実行: 未実行");

        let mut failed = BackgroundTaskState::default();
        apply_snapshot(
            &mut failed,
            ScheduleView::NotScheduled,
            LastRunView::ReadFailed("内部エラーが発生しました: background task record".to_string()),
        );
        assert!(last_run_text(&failed).contains("読み出しに失敗しました"));
        assert_ne!(last_run_text(&never), last_run_text(&failed));
    }

    #[test]
    fn format_utc_epoch() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn format_utc_leap_year_boundary() {
        // 2024-02-29 00:00:00 UTC = 1709164800000 ms
        assert_eq!(format_utc(1_709_164_800_000), "2024-02-29 00:00:00 UTC");
    }

    #[test]
    fn schedule_button_hidden_while_already_scheduled() {
        use crate::click_button;
        let mut state = BackgroundTaskState {
            schedule_view: ScheduleView::Scheduled,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "予約する", &mut state));
        assert!(click_button(&screen, "予約を取り消す", &mut state));
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = BackgroundTaskState {
            phase: Phase::InFlight,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "予約する", &mut state));
        assert!(!click_button(&screen, "予約を取り消す", &mut state));
    }

    #[test]
    fn schedule_button_clickable_when_idle_and_not_scheduled() {
        use crate::click_button;
        let mut state = BackgroundTaskState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "予約する", &mut state));
        assert_eq!(state.phase, Phase::Requested(Action::Schedule));
    }
}
