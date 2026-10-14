//! Phase 11（親 #1042）型サンプル「バックグラウンド処理」（Issue #1043）の
//! effect ドライバ。`fandhe_core::screens::background_task`（端末依存ゼロ）の
//! ボタン要求を観測して `fandhe_device::background_task::BackgroundTask` を呼び、
//! `CompletionSender` 経由で結果を返す（`crate::screens::registry` の `effect`
//! 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`device/
//! notification.rs::drive` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # ボタン要求（同期呼び出し + 別スレッド）
//!
//! `schedule()`/`cancel()` は FFI 1 回と状態更新だけで UI を要さないため、
//! `notification.rs::drive` と同じく別スレッドへ逃がして完了を送る
//! （`CompletionSender::send` 経由。UI スレッドをブロックしない）。
//!
//! # ポーリング（OS が非同期に処理を実行するため）
//!
//! OS は予約後、利用者の操作なしに処理を実行しうる。画面を開いたまま待っている
//! 利用者へ結果を見せるため、`state()`/`last_run()` を毎回読み、前回送った
//! スナップショットと差があるときだけ送信する（`device-bridge-contracts.md`
//! 「4. ポーリング・再描画」。差が無いのに毎回送ると無限再描画になる）。
//! 予約中（`ScheduleView::Scheduled`）の間だけ `request_repaint_after` で
//! 次回のポーリングを予約する（送信を省略する分岐でも pull 型は次回を予約する
//! 契約。予約が無い間は他の操作がこの画面を再描画するまで追加のポーリングは
//! 起きない。disk I/O は 1 回の `read`/`lock` 程度で軽量なため許容する）。
//!
//! # macOS・Windows・iOS・Android 実装（Issue #1043〜#1046）。Web は兄弟 issue（#1047）
//!
//! macOS・Windows・iOS・Android 以外は [`fandhe_device::background_task::UnsupportedBackgroundTask`]
//! 固定（`Unsupported` は画面側で固定文言へ畳む、[`to_message`]）。

use std::sync::Arc;
use std::time::Duration;

use fandhe_core::screens::background_task::{
    apply_snapshot, finish_action, mark_in_flight, take_request, Action, BackgroundTaskState,
    LastRunView, RunResultView, ScheduleView,
};
use fandhe_device::background_task::{BackgroundTask, RunRecord, RunResult, ScheduleState};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `ScheduleView::Scheduled` の間、次回ポーリングを予約する間隔
/// （モジュール doc「ポーリング」参照。短すぎる busy loop を避けつつ、OS 発火の
/// 検知遅延が体感で問題にならない範囲に収める）。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言のみを転送する（`DeviceError` の内部構造は漏らさない、
/// security.md A09。`notification.rs::to_message` と同型）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => "この環境ではバックグラウンド処理を使えません".to_string(),
        other => other.to_string(),
    }
}

fn to_schedule_view(state: Result<ScheduleState, DeviceError>) -> ScheduleView {
    match state {
        Ok(ScheduleState::NotScheduled) => ScheduleView::NotScheduled,
        Ok(ScheduleState::Scheduled) => ScheduleView::Scheduled,
        Ok(ScheduleState::Ran) => ScheduleView::Ran,
        // 読み取り自体の失敗は「未観測」として扱う（`last_run` 側の `ReadFailed` と
        // 異なり、予約状態の読み取り失敗を表示する専用の腕を画面側に設けていない。
        // macOS・Windows 実装の `state()` は `Mutex` ロックのみで失敗しないため
        // 実運用では到達しない防御的分岐）。
        Err(_) => ScheduleView::Unknown,
    }
}

fn to_run_result_view(result: RunResult) -> RunResultView {
    match result {
        RunResult::Completed => RunResultView::Completed,
        RunResult::Expired => RunResultView::Expired,
    }
}

fn to_last_run_view(last_run: Result<Option<RunRecord>, DeviceError>) -> LastRunView {
    match last_run {
        Ok(None) => LastRunView::NeverRan,
        Ok(Some(record)) => LastRunView::Ran {
            at_ms: record.ran_at_unix_ms,
            result: to_run_result_view(record.result),
        },
        Err(e) => LastRunView::ReadFailed(to_message(e)),
    }
}

/// iOS の `.m` 側（`ios/fandhe_ios_background_task.m::fandhe_ios_background_task_submit`）
/// が返すコードから `DeviceError` への写像（SSOT。`.m` 側で二重に写像しない、
/// 実装計画 §3.4 参照）。iOS 専用のため非 iOS の非テストでは呼び出し元が無く
/// `dead_code` になる。
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
pub(crate) fn submit_code_to_result(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::Disabled("background task")),
        2 => Err(DeviceError::PermissionDenied("background task")),
        3 => Err(DeviceError::Unsupported("background task")),
        _ => Err(DeviceError::Internal("background task schedule")),
    }
}

/// 現在のプラットフォームに応じた `BackgroundTask` 実装を返す。macOS は
/// [`super::macos_background_task::MacBackgroundTask`]（初期化失敗時は
/// `UnsupportedBackgroundTask` へ、ダミーの成功実装へは倒さない。
/// `device-bridge-contracts.md`「6. 嘘の成功」参照）。Windows は
/// [`super::windows_background_task::WindowsBackgroundTask`]（同じく初期化失敗時は
/// `UnsupportedBackgroundTask` へ。構築時に COM は呼ばないため、`FsStorage::
/// desktop_default` の失敗のみがここで倒す対象）。iOS は
/// [`super::ios_background_task::IosBackgroundTask`]、Android は
/// [`super::android_background_task::AndroidBackgroundTask`]（初期化失敗時は同様）。
/// それ以外（Web）は `UnsupportedBackgroundTask`（兄弟 issue #1047 が差し替える）。
fn implementation() -> Arc<dyn BackgroundTask> {
    #[cfg(target_os = "macos")]
    {
        use fandhe_device::storage::FsStorage;
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn BackgroundTask>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| match FsStorage::desktop_default() {
                Ok(dir) => {
                    let task = Arc::new(super::macos_background_task::MacBackgroundTask::new(
                        Arc::new(FsStorage::new(dir)),
                    ));
                    super::macos_background_task::install(task) as Arc<dyn BackgroundTask>
                }
                Err(_) => Arc::new(fandhe_device::background_task::UnsupportedBackgroundTask),
            })
            .clone()
    }
    #[cfg(target_os = "windows")]
    {
        use fandhe_device::storage::FsStorage;
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn BackgroundTask>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| match FsStorage::desktop_default() {
                Ok(dir) => Arc::new(super::windows_background_task::WindowsBackgroundTask::new(
                    Arc::new(FsStorage::new(dir)),
                )) as Arc<dyn BackgroundTask>,
                Err(_) => Arc::new(fandhe_device::background_task::UnsupportedBackgroundTask),
            })
            .clone()
    }
    #[cfg(target_os = "ios")]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn BackgroundTask>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| match super::ios_background_task::instance() {
                Some(task) => task as Arc<dyn BackgroundTask>,
                None => Arc::new(fandhe_device::background_task::UnsupportedBackgroundTask),
            })
            .clone()
    }
    #[cfg(target_os = "android")]
    {
        use fandhe_device::storage::FsStorage;
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn BackgroundTask>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| match super::android::data_dir() {
                Some(dir) => Arc::new(super::android_background_task::AndroidBackgroundTask::new(
                    Arc::new(FsStorage::new(dir)),
                )) as Arc<dyn BackgroundTask>,
                None => Arc::new(fandhe_device::background_task::UnsupportedBackgroundTask),
            })
            .clone()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn BackgroundTask>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| {
                Arc::new(fandhe_device::background_task::UnsupportedBackgroundTask)
                    as Arc<dyn BackgroundTask>
            })
            .clone()
    }
}

/// `background_task` の要求 1 件を処理して別スレッドで OS を呼び、完了を送る
/// （テストから任意の実装を差し込めるよう `background_task_effect` 本体から切り出す、
/// `notification.rs::drive` と同型）。
fn drive_request(
    state: &BackgroundTaskState,
    sender: &CompletionSender<BackgroundTaskState>,
    task: Arc<dyn BackgroundTask>,
) -> bool {
    let Some(request) = take_request(state) else {
        return false;
    };

    // 起動済みマーキング（二重起動防止、`notification.rs::drive` と同型）。
    let _ = sender.send(Box::new(mark_in_flight));

    let generation = request.generation;
    let action = request.action;
    let sender = sender.clone();
    std::thread::spawn(move || {
        let result = match action {
            Action::Schedule => task.schedule(),
            Action::Cancel => task.cancel(),
        }
        .map_err(to_message);
        let _ = sender.send(Box::new(move |s: &mut BackgroundTaskState| {
            finish_action(s, generation, action, result);
        }));
    });
    true
}

/// `background_task` のポーリング 1 回分。OS 側の予約状態・最後の実行結果を読み、
/// 画面の現在値と差があるときだけ送信する（モジュール doc「ポーリング」参照）。
/// 予約中のままなら次回ポーリングを予約する。
fn drive_poll(
    state: &BackgroundTaskState,
    sender: &CompletionSender<BackgroundTaskState>,
    task: &dyn BackgroundTask,
) {
    let schedule_view = to_schedule_view(task.state());
    let last_run_view = to_last_run_view(task.last_run());

    if schedule_view != state.schedule_view || last_run_view != state.last_run_view {
        let lrv = last_run_view;
        let _ = sender.send(Box::new(move |s: &mut BackgroundTaskState| {
            apply_snapshot(s, schedule_view, lrv);
        }));
    }

    if schedule_view == ScheduleView::Scheduled {
        sender.request_repaint_after(POLL_INTERVAL);
    }
}

/// `registry.rs` の `effect` 区画から `background_task` 画面用に配線される effect ドライバ。
pub fn background_task_effect(
    state: &BackgroundTaskState,
    sender: &CompletionSender<BackgroundTaskState>,
) {
    let task = implementation();
    if drive_request(state, sender, task.clone()) {
        return;
    }
    drive_poll(state, sender, task.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::background_task::{cancel, schedule};

    fn sender_with_channel() -> (
        CompletionSender<BackgroundTaskState>,
        std::sync::mpsc::Receiver<crate::app::Completion<BackgroundTaskState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// `schedule()`/`cancel()` の戻り値（`DeviceError` は `Clone` ではないため
    /// `Result` をそのまま保持せず、テストごとに必要な固定バリアントだけを
    /// 列挙して都度組み立てる）。
    #[derive(Clone, Copy, Default)]
    enum ActionOutcome {
        #[default]
        Ok,
        Unsupported,
    }

    impl ActionOutcome {
        fn into_result(self) -> Result<(), DeviceError> {
            match self {
                ActionOutcome::Ok => Ok(()),
                ActionOutcome::Unsupported => Err(DeviceError::Unsupported("background task")),
            }
        }
    }

    /// `last_run()` の戻り値（同じ理由で列挙から都度組み立てる）。
    #[derive(Clone, Copy, Default)]
    enum LastRunOutcome {
        #[default]
        NeverRan,
        Ran(RunRecord),
        ReadFailed,
    }

    impl LastRunOutcome {
        fn into_result(self) -> Result<Option<RunRecord>, DeviceError> {
            match self {
                LastRunOutcome::NeverRan => Ok(None),
                LastRunOutcome::Ran(record) => Ok(Some(record)),
                LastRunOutcome::ReadFailed => Err(DeviceError::Internal("background task record")),
            }
        }
    }

    /// 固定の戻り値だけを返す `BackgroundTask` モック（状態を持たず、各テストが
    /// 必要な腕だけを設定する）。
    #[derive(Clone, Copy, Default)]
    struct MockTask {
        schedule_outcome: ActionOutcome,
        cancel_outcome: ActionOutcome,
        state_value: Option<ScheduleState>,
        last_run_outcome: LastRunOutcome,
    }

    impl BackgroundTask for MockTask {
        fn schedule(&self) -> Result<(), DeviceError> {
            self.schedule_outcome.into_result()
        }
        fn cancel(&self) -> Result<(), DeviceError> {
            self.cancel_outcome.into_result()
        }
        fn state(&self) -> Result<ScheduleState, DeviceError> {
            self.state_value
                .ok_or(DeviceError::Internal("background task state"))
        }
        fn last_run(&self) -> Result<Option<RunRecord>, DeviceError> {
            self.last_run_outcome.into_result()
        }
    }

    #[test]
    fn drive_request_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = BackgroundTaskState::default();
        let task: Arc<dyn BackgroundTask> = Arc::new(MockTask {
            state_value: Some(ScheduleState::NotScheduled),
            ..Default::default()
        });
        assert!(!drive_request(&state, &sender, task));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_request_schedule_success_reports_scheduled() {
        let (sender, rx) = sender_with_channel();
        let mut state = BackgroundTaskState::default();
        schedule(&mut state);
        let task: Arc<dyn BackgroundTask> = Arc::new(MockTask::default());

        assert!(drive_request(&state, &sender, task));
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("完了が送られる")(&mut state);
        assert_eq!(
            fandhe_core::screens::background_task::status_text(&state),
            fandhe_core::screens::background_task::SCHEDULED_STATUS
        );
    }

    #[test]
    fn drive_request_unsupported_is_fixed_message() {
        let (sender, rx) = sender_with_channel();
        let mut state = BackgroundTaskState::default();
        schedule(&mut state);
        let task: Arc<dyn BackgroundTask> = Arc::new(MockTask {
            schedule_outcome: ActionOutcome::Unsupported,
            ..Default::default()
        });

        drive_request(&state, &sender, task);
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("完了が送られる")(&mut state);
        assert_eq!(
            fandhe_core::screens::background_task::error_text(&state),
            "失敗しました: この環境ではバックグラウンド処理を使えません"
        );
    }

    #[test]
    fn drive_request_cancel_without_prior_schedule_is_ok() {
        let (sender, rx) = sender_with_channel();
        let mut state = BackgroundTaskState::default();
        cancel(&mut state);
        let task: Arc<dyn BackgroundTask> = Arc::new(MockTask::default());

        drive_request(&state, &sender, task);
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("完了が送られる")(&mut state);
        assert_eq!(
            fandhe_core::screens::background_task::status_text(&state),
            fandhe_core::screens::background_task::CANCELLED_STATUS
        );
    }

    #[test]
    fn drive_poll_sends_snapshot_only_when_changed() {
        let (sender, rx) = sender_with_channel();
        let state = BackgroundTaskState::default(); // schedule_view = Unknown, last_run_view = Unloaded
        let task = MockTask {
            state_value: Some(ScheduleState::NotScheduled),
            ..Default::default()
        };

        drive_poll(&state, &sender, &task);
        let completion = rx.recv().expect("差分があるため送信される");
        let mut applied = state.clone();
        completion(&mut applied);
        assert_eq!(applied.schedule_view, ScheduleView::NotScheduled);
        assert_eq!(applied.last_run_view, LastRunView::NeverRan);

        // 2 回目は差分が無いため送信しない。
        drive_poll(&applied, &sender, &task);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_poll_reports_last_run_read_failure_distinct_from_never_ran() {
        let (sender, rx) = sender_with_channel();
        let state = BackgroundTaskState::default();
        let task = MockTask {
            state_value: Some(ScheduleState::NotScheduled),
            last_run_outcome: LastRunOutcome::ReadFailed,
            ..Default::default()
        };

        drive_poll(&state, &sender, &task);
        let completion = rx.recv().expect("差分があるため送信される");
        let mut applied = state.clone();
        completion(&mut applied);
        assert!(matches!(applied.last_run_view, LastRunView::ReadFailed(_)));
        assert_ne!(applied.last_run_view, LastRunView::NeverRan);
    }

    #[test]
    fn drive_poll_schedules_next_poll_only_while_scheduled() {
        let (sender, rx) = sender_with_channel();
        let state = BackgroundTaskState::default();
        let scheduled_task = MockTask {
            state_value: Some(ScheduleState::NotScheduled),
            ..Default::default()
        };
        drive_poll(&state, &sender, &scheduled_task);
        let completion = rx.recv().expect("差分があるため送信される");
        let mut not_scheduled = state.clone();
        completion(&mut not_scheduled);

        let scheduled = BackgroundTaskState {
            schedule_view: ScheduleView::Scheduled,
            last_run_view: LastRunView::NeverRan,
            ..Default::default()
        };
        let scheduled_task = MockTask {
            state_value: Some(ScheduleState::Scheduled),
            ..Default::default()
        };
        // 差分が無くても `Scheduled` の間は次回ポーリングを予約する契約
        // （`request_repaint_after` 自体の呼び出しは `egui::Context` 経由のため
        // ここでは「送信は起きない」ことのみを確認する）。
        drive_poll(&scheduled, &sender, &scheduled_task);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_poll_maps_ran_record_to_view() {
        let (sender, rx) = sender_with_channel();
        let state = BackgroundTaskState::default();
        let record = RunRecord {
            ran_at_unix_ms: 1_700_000_000_000,
            result: RunResult::Completed,
        };
        let task = MockTask {
            state_value: Some(ScheduleState::Ran),
            last_run_outcome: LastRunOutcome::Ran(record),
            ..Default::default()
        };

        drive_poll(&state, &sender, &task);
        let completion = rx.recv().expect("差分があるため送信される");
        let mut applied = state.clone();
        completion(&mut applied);
        assert_eq!(applied.schedule_view, ScheduleView::Ran);
        assert_eq!(
            applied.last_run_view,
            LastRunView::Ran {
                at_ms: record.ran_at_unix_ms,
                result: RunResultView::Completed,
            }
        );
    }
}
