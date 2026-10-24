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
//! （`device-bridge-contracts.md`「4. ポーリング・再描画」）。予約中の間だけ
//! `request_repaint_after` で次回を予約し、予約が無い間は追加のポーリングを起こさない。
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
    apply_snapshot, finish_action, mark_in_flight, take_request, Action, AlarmShowState, Phase,
    ScheduleView,
};
use fandhe_device::alarm::{Alarm, AlarmState};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 予約中の間、次回ポーリングを予約する間隔。
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

fn to_schedule_view(state: Result<AlarmState, DeviceError>) -> ScheduleView {
    match state {
        Ok(AlarmState::NotScheduled) => ScheduleView::NotScheduled,
        Ok(AlarmState::Scheduled { fire_at_unix_ms }) => {
            ScheduleView::Scheduled { fire_at_unix_ms }
        }
        // 読み取りの失敗は「未観測」として扱い、成功値へ畳まない。
        Err(_) => ScheduleView::Unknown,
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
) {
    if state.phase != Phase::Idle || in_flight.swap(true, Ordering::AcqRel) {
        return;
    }
    let current = state.schedule_view;
    let generation = state.generation;
    let sender = sender.clone();
    std::thread::spawn(move || {
        if poll_once(alarm.as_ref(), current, generation, &sender) {
            sender.request_repaint_after(POLL_INTERVAL);
        }
        in_flight.store(false, Ordering::Release);
    });
}

/// 予約一覧を 1 回読み、画面の値と差があれば世代付きで送る。次回ポーリングを
/// 予約すべきなら真を返す（予約中、または今回の送信で画面の値が変わったとき。
/// `NotScheduled` を適用した直後も 1 回確認し直す、契約表 4）。
fn poll_once(
    alarm: &dyn Alarm,
    current: ScheduleView,
    generation: u64,
    sender: &CompletionSender<AlarmShowState>,
) -> bool {
    let view = to_schedule_view(alarm.state());
    let changed = view != current;
    if changed {
        let _ = sender.send(Box::new(move |s: &mut AlarmShowState| {
            apply_snapshot(s, view, generation);
        }));
    }
    changed || matches!(view, ScheduleView::Scheduled { .. })
}

/// ポーリングの多重起動防止フラグ（プロセス内で 1 本）。
fn poll_in_flight() -> Arc<AtomicBool> {
    use std::sync::OnceLock;
    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// 前回のポーリング開始から [`POLL_INTERVAL`] 経ったときだけ真を返し、時刻を更新する。
/// 単調時計（`Instant`）で測る。フレームごとに呼ばれても OS 呼び出しを間引く。
fn poll_due() -> bool {
    use std::sync::OnceLock;
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let Ok(mut last) = LAST.get_or_init(|| Mutex::new(None)).lock() else {
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
    let alarm = implementation();
    if drive_request(state, sender, alarm.clone(), now_unix_ms) {
        return;
    }
    if poll_due() {
        drive_poll(state, sender, alarm, poll_in_flight());
    } else if matches!(state.schedule_view, ScheduleView::Scheduled { .. }) {
        // 間引いたフレームでも、予約中は次回の確認を取りこぼさない。
        sender.request_repaint_after(POLL_INTERVAL);
    }
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
        drive_poll(&state, &sender, alarm.clone(), flag.clone());
        let completion = rx.recv().expect("差分があるため送信される");
        let mut applied = state.clone();
        completion(&mut applied);
        assert_eq!(applied.schedule_view, ScheduleView::NotScheduled);
        wait_idle(&flag);

        drive_poll(&applied, &sender, alarm, flag.clone());
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
        drive_poll(&state, &sender, alarm, flag.clone());
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
        assert!(poll_once(&fake, current, 0, &sender));
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
        );
        std::thread::sleep(Duration::from_millis(50));
        assert!(!flag.load(Ordering::Acquire));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn poll_requests_next_poll_after_applying_not_scheduled() {
        let (sender, _rx) = sender_with_channel();
        let scheduled = ScheduleView::Scheduled { fire_at_unix_ms: 9 };
        // Scheduled -> NotScheduled の差分を送った回は次回ポーリングを予約する。
        let gone = fake_with_state(AlarmState::NotScheduled);
        assert!(poll_once(&gone, scheduled, 0, &sender));
        // 差分が無い未予約は予約しない（予約が無い間は追加のポーリングを起こさない）。
        assert!(!poll_once(&gone, ScheduleView::NotScheduled, 0, &sender));
        // 予約中のままなら予約する。
        let still = fake_with_state(AlarmState::Scheduled { fire_at_unix_ms: 9 });
        assert!(poll_once(&still, scheduled, 0, &sender));
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
        drive_poll(&state, &sender, alarm, flag);
        std::thread::sleep(Duration::from_millis(50));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn poll_read_failure_is_unknown_not_scheduled() {
        assert_eq!(
            to_schedule_view(Err(DeviceError::Internal("alarm list"))),
            ScheduleView::Unknown
        );
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
