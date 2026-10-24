//! Phase 11（親 #140、Issue #721）型サンプル「アラーム・正確な時刻の予約」の
//! effect ドライバ。`fandhe_core::screens::alarm_show`（端末依存ゼロ）のボタン要求を
//! 観測して `fandhe_device::alarm::Alarm` を呼び、`CompletionSender` 経由で結果を
//! 返す（`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)`
//! として呼ばれる契約、`device/background_task.rs` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # ボタン要求（別スレッド）
//!
//! OS 呼び出し（COM・予約一覧の読み書き）は UI スレッドをブロックしないよう別
//! スレッドへ逃がし、完了を送る。「1 分後」の絶対時刻は現在時刻の取得を含めて
//! このスレッド内で作る（`fandhe-core` は wasm32 で panic する `SystemTime` を
//! 使えないため、現在時刻は proxy 側の責務）。
//!
//! # ポーリング
//!
//! 時刻が来ると OS が利用者の操作なしに予約を一覧から外す。画面を開いたまま待つ
//! 利用者へ見せるため `state()` を読み、画面の値と差があるときだけ送る
//! （`device-bridge-contracts.md`「4. ポーリング・再描画」）。予約は画面外・別アプリ
//! からも変わりうるため、表示内容（未予約・未観測・操作失敗後を含む）で再確認の要否を
//! 絞らず、画面が表示されている間は [`POLL_INTERVAL`] ごとに OS を確かめ直す。止めるのは
//! 回復しない `Unsupported`（[`Observation::Unavailable`]）のときだけ。操作の完了直後は
//! 次の間隔を待たずに確かめ直す。取得の失敗は直前の確定表示を消さず、固定文言（OS のエラー文は出さない）
//! を併記して確認を続け、回復したら自動で正しい状態へ戻す。
//!
//! # 実装
//!
//! Windows は [`super::windows_alarm::WindowsAlarm`]。それ以外は
//! [`fandhe_device::alarm::UnsupportedAlarm`] 固定で、`Unsupported` は固定文言に
//! 畳む（[`to_message`]）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fandhe_core::screens::alarm_show::{
    apply_check_failure, apply_snapshot, finish_action, mark_in_flight, take_request, Action,
    AlarmShowState, Phase, ScheduleView,
};
use fandhe_device::alarm::{Alarm, AlarmState};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 画面の表示中、OS の予約状態を確かめ直す間隔。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 「1 分後」の長さ。
const DELAY: Duration = Duration::from_secs(60);

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言のみを転送する（内部構造・パスは漏らさない、
/// security.md A09）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => "この環境ではアラームを予約できません".to_string(),
        other => other.to_string(),
    }
}

/// 予約一覧の読み取り結果。失敗は成功値へ畳まず、直前の確定表示も消さない
/// 別の区分にする（`device-bridge-contracts.md`「6. 嘘の成功」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Observation {
    /// 読み取れた。
    View(ScheduleView),
    /// 一時的に読み取れなかった。画面へは固定文言で併記し、確認を続ける。
    Failed,
    /// この環境では読めない（`Unsupported`）。回復しないため確認を続けない。
    Unavailable,
}

fn to_observation(state: Result<AlarmState, DeviceError>) -> Observation {
    match state {
        Ok(AlarmState::NotScheduled) => Observation::View(ScheduleView::NotScheduled),
        Ok(AlarmState::Scheduled { fire_at_unix_ms }) => {
            Observation::View(ScheduleView::Scheduled { fire_at_unix_ms })
        }
        Err(DeviceError::Unsupported(_)) => Observation::Unavailable,
        // OS のエラー文は画面へ出さない（固定文言のみ）。
        Err(_) => Observation::Failed,
    }
}

/// 現在の壁時計（UNIX ms）。エポックより前・範囲外は `Internal`。
fn now_unix_ms() -> Result<u64, DeviceError> {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DeviceError::Internal("alarm clock"))?;
    u64::try_from(d.as_millis()).map_err(|_| DeviceError::Internal("alarm clock"))
}

/// 現在のプラットフォームに応じた `Alarm` 実装を返す。Windows は
/// [`super::windows_alarm::WindowsAlarm`]、それ以外は `UnsupportedAlarm`
/// （ダミーの成功実装へは倒さない。`device-bridge-contracts.md`「6. 嘘の成功」）。
fn implementation() -> Arc<dyn Alarm> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn Alarm>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                Arc::new(super::windows_alarm::WindowsAlarm::new()) as Arc<dyn Alarm>
            }
            #[cfg(not(target_os = "windows"))]
            {
                Arc::new(fandhe_device::alarm::UnsupportedAlarm) as Arc<dyn Alarm>
            }
        })
        .clone()
}

/// 要求 1 件を処理して別スレッドで OS を呼び、完了を送る（テストから任意の実装を
/// 差し込めるよう `alarm_show_effect` 本体から切り出す）。
fn drive_request(
    state: &AlarmShowState,
    sender: &CompletionSender<AlarmShowState>,
    alarm: Arc<dyn Alarm>,
    now: fn() -> Result<u64, DeviceError>,
) -> bool {
    let Some(request) = take_request(state) else {
        return false;
    };

    // 起動済みマーキング（二重起動防止）。
    let _ = sender.send(Box::new(mark_in_flight));

    let generation = request.generation;
    let action = request.action;
    let sender = sender.clone();
    std::thread::spawn(move || {
        let result: Result<Option<u64>, DeviceError> = match action {
            Action::Schedule => now().and_then(|n| {
                let fire_at = n.saturating_add(DELAY.as_millis() as u64);
                alarm.schedule(fire_at).map(|()| Some(fire_at))
            }),
            Action::Cancel => alarm.cancel().map(|()| None),
        };
        let result = result.map_err(to_message);
        let _ = sender.send(Box::new(move |s: &mut AlarmShowState| {
            finish_action(s, generation, action, result);
            // 完了直後は間隔を待たず、次のフレームで OS の実状態を確かめ直す。
            reset_poll_due();
        }));
    });
    true
}

/// ポーリング 1 回分。OS の予約一覧の読み取り（COM・ファイル I/O を含みうる）は
/// UI スレッドで行わず、別スレッドへ逃がす。`in_flight` が立っている間は新しい
/// スレッドを起こさない（多重起動防止）。操作中（`Idle` 以外）は読み始めない。
/// 読み始めた時点の世代を snapshot に持たせ、読む間に予約・取り消しが挟まった
/// 古い観測は適用側（`apply_snapshot`）で捨てる。
fn drive_poll(
    state: &AlarmShowState,
    sender: &CompletionSender<AlarmShowState>,
    alarm: Arc<dyn Alarm>,
    in_flight: Arc<AtomicBool>,
    unavailable: Arc<AtomicBool>,
) {
    if state.phase != Phase::Idle || in_flight.swap(true, Ordering::AcqRel) {
        return;
    }
    let current = state.schedule_view;
    let check_failed = state.check_failed;
    let generation = state.generation;
    let sender = sender.clone();
    std::thread::spawn(move || {
        if !poll_once(alarm.as_ref(), current, check_failed, generation, &sender) {
            unavailable.store(true, Ordering::Release);
        }
        in_flight.store(false, Ordering::Release);
    });
}

/// 予約一覧を 1 回読み、画面の値と差があれば世代付きで送る。確認を続けるべきなら
/// 真を返す。偽は回復しない `Unavailable` のときだけで、表示内容では絞らない。
/// 取得の失敗は `schedule_view` を上書きせず、確認失敗の印だけを送る
/// （既に印が立っていれば再送しないが、OS が回復するまで確認は続ける）。
fn poll_once(
    alarm: &dyn Alarm,
    current: ScheduleView,
    current_check_failed: bool,
    generation: u64,
    sender: &CompletionSender<AlarmShowState>,
) -> bool {
    match to_observation(alarm.state()) {
        Observation::View(view) => {
            if view != current || current_check_failed {
                let _ = sender.send(Box::new(move |s: &mut AlarmShowState| {
                    apply_snapshot(s, view, generation);
                }));
            }
            true
        }
        Observation::Failed => {
            if !current_check_failed {
                let _ = sender.send(Box::new(move |s: &mut AlarmShowState| {
                    apply_check_failure(s, generation);
                }));
            }
            true
        }
        Observation::Unavailable => {
            // 従来どおり未観測として扱う（再確認しても回復しない）。
            if current != ScheduleView::Unknown {
                let _ = sender.send(Box::new(move |s: &mut AlarmShowState| {
                    apply_snapshot(s, ScheduleView::Unknown, generation);
                }));
            }
            false
        }
    }
}

/// 回復しない `Unsupported` を観測した印（プロセス内で 1 本）。立てば確認を止める。
fn poll_unavailable() -> Arc<AtomicBool> {
    use std::sync::OnceLock;
    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// ポーリングの多重起動防止フラグ（プロセス内で 1 本）。
fn poll_in_flight() -> Arc<AtomicBool> {
    use std::sync::OnceLock;
    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// 前回のポーリング開始時刻（単調時計）。
fn poll_last() -> &'static Mutex<Option<Instant>> {
    use std::sync::OnceLock;
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

/// 次の [`poll_due`] を真にする（操作の完了直後に確かめ直すため）。
fn reset_poll_due() {
    if let Ok(mut last) = poll_last().lock() {
        *last = None;
    }
}

/// 前回のポーリング開始から [`POLL_INTERVAL`] 経ったときだけ真を返し、時刻を更新する。
/// 単調時計（`Instant`）で測る。フレームごとに呼ばれても OS 呼び出しを間引く。
fn poll_due() -> bool {
    let Ok(mut last) = poll_last().lock() else {
        return false;
    };
    let now = Instant::now();
    match *last {
        Some(t) if now.duration_since(t) < POLL_INTERVAL => false,
        _ => {
            *last = Some(now);
            true
        }
    }
}

/// `registry.rs` の `effect` 区画から `alarm_show` 画面用に配線される effect ドライバ。
pub fn alarm_show_effect(state: &AlarmShowState, sender: &CompletionSender<AlarmShowState>) {
    drive_effect(
        state,
        sender,
        implementation(),
        now_unix_ms,
        poll_due,
        poll_in_flight(),
        poll_unavailable(),
    );
}

/// `alarm_show_effect` の本体（テストから実装・時計・間引きを差し込む）。
/// 要求があれば処理する（完了が再描画を起こす）。無ければ、回復しない `Unsupported`
/// を観測済みでない限り、表示内容に関係なく毎回次回の確認を予約し、間隔が来ていれば
/// 読みに行く。
fn drive_effect(
    state: &AlarmShowState,
    sender: &CompletionSender<AlarmShowState>,
    alarm: Arc<dyn Alarm>,
    now: fn() -> Result<u64, DeviceError>,
    due: fn() -> bool,
    in_flight: Arc<AtomicBool>,
    unavailable: Arc<AtomicBool>,
) {
    if drive_request(state, sender, alarm.clone(), now) {
        return;
    }
    if unavailable.load(Ordering::Acquire) {
        return;
    }
    if due() {
        drive_poll(state, sender, alarm, in_flight, unavailable);
    }
    sender.request_repaint_after(POLL_INTERVAL);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::alarm_show::{
        cancel, error_text, schedule, status_text, CANCELLED_STATUS, SCHEDULED_STATUS,
    };

    fn sender_with_channel() -> (
        CompletionSender<AlarmShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<AlarmShowState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// 呼び出しごとの結果を固定する偽実装（`DeviceError` は `Clone` ではないため
    /// 固定バリアントから都度組み立てる）。
    #[derive(Clone, Copy)]
    enum Outcome {
        Ok,
        Unsupported,
        PermissionDenied,
        InvalidInput,
    }

    impl Outcome {
        fn into_result(self) -> Result<(), DeviceError> {
            match self {
                Outcome::Ok => Ok(()),
                Outcome::Unsupported => Err(DeviceError::Unsupported("alarm")),
                Outcome::PermissionDenied => Err(DeviceError::PermissionDenied("alarm")),
                Outcome::InvalidInput => Err(DeviceError::InvalidInput("alarm time")),
            }
        }
    }

    struct Fake {
        schedule: Outcome,
        cancel: Outcome,
        state: Option<AlarmState>,
    }

    impl Alarm for Fake {
        fn schedule(&self, _fire_at_unix_ms: u64) -> Result<(), DeviceError> {
            self.schedule.into_result()
        }
        fn cancel(&self) -> Result<(), DeviceError> {
            self.cancel.into_result()
        }
        fn state(&self) -> Result<AlarmState, DeviceError> {
            self.state.ok_or(DeviceError::Internal("alarm list"))
        }
    }

    fn fake(schedule: Outcome) -> Arc<dyn Alarm> {
        Arc::new(Fake {
            schedule,
            cancel: Outcome::Ok,
            state: Some(AlarmState::NotScheduled),
        })
    }

    fn fixed_now() -> Result<u64, DeviceError> {
        Ok(1_700_000_000_000)
    }

    fn run_schedule(alarm: Arc<dyn Alarm>) -> AlarmShowState {
        let (sender, rx) = sender_with_channel();
        let mut state = AlarmShowState::default();
        schedule(&mut state);
        assert!(drive_request(&state, &sender, alarm, fixed_now));
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("完了が送られる")(&mut state);
        state
    }

    #[test]
    fn noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = AlarmShowState::default();
        assert!(!drive_request(
            &state,
            &sender,
            fake(Outcome::Ok),
            fixed_now
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn schedule_success_reports_one_minute_later() {
        let state = run_schedule(fake(Outcome::Ok));
        assert_eq!(status_text(&state), SCHEDULED_STATUS);
        assert_eq!(
            state.schedule_view,
            ScheduleView::Scheduled {
                fire_at_unix_ms: 1_700_000_060_000
            }
        );
    }

    #[test]
    fn unsupported_os_never_reports_success() {
        let state = run_schedule(Arc::new(fandhe_device::alarm::UnsupportedAlarm));
        assert_eq!(status_text(&state), "");
        assert_eq!(
            error_text(&state),
            "失敗しました: この環境ではアラームを予約できません"
        );
        assert_eq!(state.schedule_view, ScheduleView::Unknown);
    }

    #[test]
    fn permission_denied_reaches_screen_as_failure() {
        let state = run_schedule(fake(Outcome::PermissionDenied));
        assert!(error_text(&state).starts_with("失敗しました: "));
        assert_eq!(status_text(&state), "");
    }

    #[test]
    fn invalid_input_reaches_screen_as_failure() {
        let state = run_schedule(fake(Outcome::InvalidInput));
        assert!(error_text(&state).starts_with("失敗しました: "));
        assert!(!matches!(
            state.schedule_view,
            ScheduleView::Scheduled { .. }
        ));
    }

    #[test]
    fn clock_failure_is_a_failure_not_a_schedule() {
        fn broken_now() -> Result<u64, DeviceError> {
            Err(DeviceError::Internal("alarm clock"))
        }
        let (sender, rx) = sender_with_channel();
        let mut state = AlarmShowState::default();
        schedule(&mut state);
        drive_request(&state, &sender, fake(Outcome::Ok), broken_now);
        rx.recv().expect("起動済みマーク")(&mut state);
        rx.recv().expect("完了")(&mut state);
        assert!(error_text(&state).starts_with("失敗しました: "));
    }

    #[test]
    fn stale_generation_completion_does_not_become_success() {
        let (sender, rx) = sender_with_channel();
        let mut state = AlarmShowState::default();
        schedule(&mut state); // generation 1
        drive_request(&state, &sender, fake(Outcome::Ok), fixed_now);
        rx.recv().expect("起動済みマーク")(&mut state);
        // 完了が届く前に新しい要求（世代 2）へ差し替わる状況を作る。
        state.phase = fandhe_core::screens::alarm_show::Phase::Idle;
        cancel(&mut state);
        let before = state.clone();
        rx.recv().expect("世代 1 の完了")(&mut state);
        assert_eq!(state, before);
    }

    #[test]
    fn cancel_is_reported_as_success() {
        let (sender, rx) = sender_with_channel();
        let mut state = AlarmShowState::default();
        cancel(&mut state);
        drive_request(&state, &sender, fake(Outcome::Ok), fixed_now);
        rx.recv().expect("起動済みマーク")(&mut state);
        rx.recv().expect("完了")(&mut state);
        assert_eq!(status_text(&state), CANCELLED_STATUS);
        assert_eq!(error_text(&state), "");
    }

    #[test]
    fn poll_sends_only_on_difference() {
        let (sender, rx) = sender_with_channel();
        let state = AlarmShowState::default(); // Unknown
        let alarm: Arc<dyn Alarm> = Arc::new(Fake {
            schedule: Outcome::Ok,
            cancel: Outcome::Ok,
            state: Some(AlarmState::NotScheduled),
        });
        let flag = Arc::new(AtomicBool::new(false));
        drive_poll(&state, &sender, alarm.clone(), flag.clone(), no());
        let completion = rx.recv().expect("差分があるため送信される");
        let mut applied = state.clone();
        completion(&mut applied);
        assert_eq!(applied.schedule_view, ScheduleView::NotScheduled);
        wait_idle(&flag);

        drive_poll(&applied, &sender, alarm, flag.clone(), no());
        wait_idle(&flag);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn poll_does_not_send_while_scheduled_and_unchanged() {
        let (sender, rx) = sender_with_channel();
        let state = AlarmShowState {
            schedule_view: ScheduleView::Scheduled { fire_at_unix_ms: 9 },
            ..Default::default()
        };
        let alarm: Arc<dyn Alarm> = Arc::new(Fake {
            schedule: Outcome::Ok,
            cancel: Outcome::Ok,
            state: Some(AlarmState::Scheduled { fire_at_unix_ms: 9 }),
        });
        // 差分が無ければ送らない（次回ポーリングの予約は `request_repaint_after` 側）。
        let flag = Arc::new(AtomicBool::new(false));
        drive_poll(&state, &sender, alarm, flag.clone(), no());
        wait_idle(&flag);
        assert!(rx.try_recv().is_err());
    }

    fn fake_with_state(state: AlarmState) -> Fake {
        Fake {
            schedule: Outcome::Ok,
            cancel: Outcome::Ok,
            state: Some(state),
        }
    }

    #[test]
    fn poll_snapshot_read_before_a_newer_action_is_discarded() {
        let (sender, rx) = sender_with_channel();
        // 世代 0 で読み始めたポーリング（OS はまだ予約を返さない）。
        let fake = fake_with_state(AlarmState::NotScheduled);
        let current = ScheduleView::Scheduled { fire_at_unix_ms: 9 };
        assert!(poll_once(&fake, current, false, 0, &sender));
        let stale = rx.recv().expect("差分があるため送信される");
        // 読む間に予約が完了して世代 1・Idle になった画面へ、古い snapshot が届く。
        let mut state = AlarmShowState::default();
        schedule(&mut state);
        mark_in_flight(&mut state);
        finish_action(&mut state, 1, Action::Schedule, Ok(Some(9)));
        stale(&mut state);
        assert_eq!(state.schedule_view, current);
        assert_eq!(status_text(&state), SCHEDULED_STATUS);
    }

    #[test]
    fn poll_snapshot_while_action_pending_is_discarded() {
        let (sender, rx) = sender_with_channel();
        let fake = fake_with_state(AlarmState::NotScheduled);
        let mut state = AlarmShowState {
            schedule_view: ScheduleView::Scheduled { fire_at_unix_ms: 9 },
            ..Default::default()
        };
        assert!(poll_once(
            &fake,
            state.schedule_view,
            false,
            state.generation,
            &sender
        ));
        let snapshot = rx.recv().expect("送信される");
        cancel(&mut state); // 読む間に取り消しが要求された
        snapshot(&mut state);
        assert_eq!(
            state.schedule_view,
            ScheduleView::Scheduled { fire_at_unix_ms: 9 }
        );
    }

    #[test]
    fn poll_is_not_started_while_action_pending() {
        let (sender, rx) = sender_with_channel();
        let mut state = AlarmShowState::default();
        schedule(&mut state);
        let flag = Arc::new(AtomicBool::new(false));
        drive_poll(
            &state,
            &sender,
            Arc::new(fake_with_state(AlarmState::NotScheduled)),
            flag.clone(),
            no(),
        );
        std::thread::sleep(Duration::from_millis(50));
        assert!(!flag.load(Ordering::Acquire));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn poll_keeps_going_regardless_of_displayed_value() {
        let (sender, _rx) = sender_with_channel();
        let scheduled = ScheduleView::Scheduled { fire_at_unix_ms: 9 };
        let gone = fake_with_state(AlarmState::NotScheduled);
        assert!(poll_once(&gone, scheduled, false, 0, &sender));
        // 差分が無い未予約でも確認を続ける（予約は画面外からも変わりうる）。
        assert!(poll_once(
            &gone,
            ScheduleView::NotScheduled,
            false,
            0,
            &sender
        ));
        let still = fake_with_state(AlarmState::Scheduled { fire_at_unix_ms: 9 });
        assert!(poll_once(&still, scheduled, false, 0, &sender));
    }

    fn no() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn always_due() -> bool {
        true
    }

    fn never_due() -> bool {
        false
    }

    type RecordingSender = (
        CompletionSender<AlarmShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<AlarmShowState>>,
        Arc<Mutex<Vec<Duration>>>,
    );

    /// 予約された遅延が次回確認の予約（0 より大きく [`POLL_INTERVAL`] 以下）か。
    /// egui は直近フレーム時間ぶん遅延を縮め、最小値を更新したときだけ通知する。
    fn has_poll_reservation(delays: &Mutex<Vec<Duration>>) -> bool {
        delays
            .lock()
            .unwrap()
            .iter()
            .any(|d| *d > Duration::ZERO && *d <= POLL_INTERVAL)
    }

    /// `request_repaint_after` で予約された遅延を記録するコンテキスト付き sender。
    fn sender_recording_delays() -> RecordingSender {
        let ctx = egui::Context::default();
        let delays = Arc::new(Mutex::new(Vec::new()));
        let rec = delays.clone();
        ctx.set_request_repaint_callback(move |info| rec.lock().unwrap().push(info.delay));
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx, delays)
    }

    #[test]
    fn next_poll_is_scheduled_even_when_not_scheduled_after_failed_action() {
        // 操作が失敗し、表示は NotScheduled のまま（エラーは保持）。
        let mut state = AlarmShowState::default();
        schedule(&mut state);
        mark_in_flight(&mut state);
        finish_action(&mut state, 1, Action::Schedule, Err("x".to_string()));
        let generation = state.generation;
        apply_snapshot(&mut state, ScheduleView::NotScheduled, generation);
        assert_eq!(state.schedule_view, ScheduleView::NotScheduled);
        assert!(!state.check_failed);
        // 間隔が来ていないフレームでも、来ているフレームでも次回を予約する。
        for due in [never_due as fn() -> bool, always_due] {
            let (sender, _rx, delays) = sender_recording_delays();
            drive_effect(
                &state,
                &sender,
                fake(Outcome::Ok),
                fixed_now,
                due,
                no(),
                no(),
            );
            assert!(has_poll_reservation(&delays));
        }
    }

    #[test]
    fn next_poll_is_scheduled_while_unknown() {
        let (sender, _rx, delays) = sender_recording_delays();
        let state = AlarmShowState::default();
        drive_effect(
            &state,
            &sender,
            fake(Outcome::Ok),
            fixed_now,
            never_due,
            no(),
            no(),
        );
        assert!(has_poll_reservation(&delays));
    }

    #[test]
    fn no_poll_is_scheduled_once_unavailable() {
        let (sender, _rx, delays) = sender_recording_delays();
        let state = AlarmShowState::default();
        drive_effect(
            &state,
            &sender,
            fake(Outcome::Ok),
            fixed_now,
            always_due,
            no(),
            Arc::new(AtomicBool::new(true)),
        );
        assert!(!has_poll_reservation(&delays));
    }

    #[test]
    fn unsupported_poll_marks_unavailable() {
        let (sender, _rx) = sender_with_channel();
        let state = AlarmShowState::default();
        let flag = no();
        let unavailable = no();
        drive_poll(
            &state,
            &sender,
            Arc::new(fandhe_device::alarm::UnsupportedAlarm),
            flag.clone(),
            unavailable.clone(),
        );
        wait_idle(&flag);
        assert!(unavailable.load(Ordering::Acquire));
    }

    #[test]
    fn action_completion_makes_next_poll_due() {
        let (sender, rx) = sender_with_channel();
        let mut state = AlarmShowState::default();
        schedule(&mut state);
        drive_request(&state, &sender, fake(Outcome::Ok), fixed_now);
        rx.recv().expect("起動済みマーク")(&mut state);
        // 他テストの完了も同じ時刻を初期化するため、完了後に真になることだけ確かめる。
        let _ = poll_due();
        rx.recv().expect("完了")(&mut state);
        assert!(poll_due());
    }

    /// ポーリングスレッドの終了（フラグ解除）を待つ。
    fn wait_idle(flag: &AtomicBool) {
        let start = Instant::now();
        while flag.load(Ordering::Acquire) {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "ポーリングが終わらない"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn poll_is_not_started_twice_while_in_flight() {
        let (sender, rx) = sender_with_channel();
        let state = AlarmShowState::default();
        let alarm: Arc<dyn Alarm> = Arc::new(Fake {
            schedule: Outcome::Ok,
            cancel: Outcome::Ok,
            state: Some(AlarmState::NotScheduled),
        });
        let flag = Arc::new(AtomicBool::new(true));
        drive_poll(&state, &sender, alarm, flag, no());
        std::thread::sleep(Duration::from_millis(50));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn read_failure_is_failed_not_unknown_or_scheduled() {
        assert_eq!(
            to_observation(Err(DeviceError::Internal("alarm list"))),
            Observation::Failed
        );
        assert_eq!(
            to_observation(Err(DeviceError::Unsupported("alarm"))),
            Observation::Unavailable
        );
    }

    fn failing() -> Fake {
        Fake {
            schedule: Outcome::Ok,
            cancel: Outcome::Ok,
            state: None,
        }
    }

    #[test]
    fn poll_failure_keeps_scheduled_view_and_keeps_polling() {
        let (sender, rx) = sender_with_channel();
        let scheduled = ScheduleView::Scheduled { fire_at_unix_ms: 9 };
        let mut state = AlarmShowState {
            schedule_view: scheduled,
            ..Default::default()
        };
        // 1 回目の失敗: 印を送り、次回も予約する。
        assert!(poll_once(&failing(), scheduled, false, 0, &sender));
        rx.recv().expect("確認失敗が送られる")(&mut state);
        assert_eq!(state.schedule_view, scheduled);
        assert!(state.check_failed);
        // 2 回続けて失敗しても、再送せずポーリングは止めない。
        assert!(poll_once(&failing(), scheduled, true, 0, &sender));
        assert!(rx.try_recv().is_err());
        // 未観測のまま失敗し続けても止めない。
        assert!(poll_once(
            &failing(),
            ScheduleView::Unknown,
            true,
            0,
            &sender
        ));
    }

    #[test]
    fn poll_recovers_after_failure_even_without_view_change() {
        let (sender, rx) = sender_with_channel();
        let scheduled = ScheduleView::Scheduled { fire_at_unix_ms: 9 };
        let mut state = AlarmShowState {
            schedule_view: scheduled,
            check_failed: true,
            ..Default::default()
        };
        let ok = fake_with_state(AlarmState::Scheduled { fire_at_unix_ms: 9 });
        assert!(poll_once(&ok, scheduled, true, 0, &sender));
        rx.recv().expect("回復が送られる")(&mut state);
        assert!(!state.check_failed);
        assert_eq!(state.schedule_view, scheduled);
    }

    #[test]
    fn poll_recovery_shows_cleared_when_reservation_vanished_during_failure() {
        let (sender, rx) = sender_with_channel();
        let scheduled = ScheduleView::Scheduled { fire_at_unix_ms: 9 };
        let mut state = AlarmShowState {
            schedule_view: scheduled,
            check_failed: true,
            ..Default::default()
        };
        let gone = fake_with_state(AlarmState::NotScheduled);
        assert!(poll_once(&gone, scheduled, true, 0, &sender));
        rx.recv().expect("送られる")(&mut state);
        assert_eq!(state.schedule_view, ScheduleView::NotScheduled);
        assert_eq!(
            state.status,
            fandhe_core::screens::alarm_show::CLEARED_STATUS
        );
    }

    #[test]
    fn stale_check_failure_is_discarded() {
        let (sender, rx) = sender_with_channel();
        assert!(poll_once(
            &failing(),
            ScheduleView::NotScheduled,
            false,
            0,
            &sender
        ));
        let stale = rx.recv().expect("送られる");
        let mut state = AlarmShowState::default();
        schedule(&mut state);
        mark_in_flight(&mut state);
        finish_action(&mut state, 1, Action::Schedule, Ok(Some(9)));
        stale(&mut state);
        assert!(!state.check_failed);
    }

    #[test]
    fn unsupported_state_does_not_poll_forever() {
        let (sender, rx) = sender_with_channel();
        let alarm = fandhe_device::alarm::UnsupportedAlarm;
        assert!(!poll_once(&alarm, ScheduleView::Unknown, false, 0, &sender));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unsupported_message_is_fixed() {
        assert_eq!(
            to_message(DeviceError::Unsupported("alarm")),
            "この環境ではアラームを予約できません"
        );
        let _ = Outcome::Unsupported.into_result();
    }
}
