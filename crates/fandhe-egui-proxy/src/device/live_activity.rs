//! Phase 11 の拡充ループ（親 #88、Issue #294）型サンプル「ライブアクティビティ」の effect ドライバ。
//! `fandhe_core::screens::live_activity_show`（端末依存ゼロ）の要求を観測し、値を
//! `fandhe_device::live_activity::LiveValue` で検査してから OS へ渡し、結果を `CompletionSender`
//! 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は
//! `fandhe_device::live_activity` のモジュール doc を参照。
//!
//! # dispatch（最終形）
//!
//! [`platform_live_activity`] が OS ごとに実装を選ぶ。iOS は `ios_live_activity::implementation()`、
//! Android は `android_live_activity::implementation()`、それ以外（macOS・Windows・Linux）は
//! `UnsupportedLiveActivity` 固定。**後続の b 層は各 OS モジュールの `implementation()` だけを埋め**、
//! 本ファイル・`device.rs` の `pub mod` 列・共通コア画面には触らない。実装は完了時に
//! [`bridge::deliver`] を 1 回だけ呼ぶ（pull 型。`request_slot::RequestSlot` の facade）。
//!
//! # 並行性
//!
//! 要求の照合（現在の `request_id` か・結果済みか）は `RequestSlot` のロックの内側で行い、
//! OS 呼び出し（`submit`）はロックの外で行う。処理中（結果未着）に届いた要求は二重に渡さず、完了後に
//! 取り込む。

use std::sync::{Arc, OnceLock};

use fandhe_core::screens::live_activity_show::{
    apply_result, mark_command_taken, take_command, LiveActivityShowState, LiveCommand, LiveResult,
};
use fandhe_device::live_activity::{LiveActivity, LiveOperation, LiveOutcome, LiveValue};
use fandhe_device::DeviceError;

use super::request_slot::{poll_and_finish, RequestSlot, POLL_INTERVAL};
use crate::app::CompletionSender;

/// OS ごとの実装を 1 つだけ持つ（iOS・Android は各 OS 実装、他 OS は非対応）。
fn platform_live_activity() -> &'static Arc<dyn LiveActivity> {
    static INSTANCE: OnceLock<Arc<dyn LiveActivity>> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        #[cfg(target_os = "ios")]
        return super::ios_live_activity::implementation();
        #[cfg(target_os = "android")]
        return super::android_live_activity::implementation();
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        return Arc::new(fandhe_device::live_activity::UnsupportedLiveActivity);
    })
}

/// `DeviceError` / 成功結果を画面用の結果へ写像する。`Internal` と未知のエラー（`Cancelled` を含む）は
/// 固定文言の失敗にし、OS の文言は入れない（security.md A09）。
fn to_result(result: Result<LiveOutcome, DeviceError>) -> LiveResult {
    match result {
        Ok(LiveOutcome::Started) => LiveResult::Started,
        Ok(LiveOutcome::Updated) => LiveResult::Updated,
        Ok(LiveOutcome::Ended) => LiveResult::Ended,
        Ok(LiveOutcome::Dismissed) => LiveResult::Dismissed,
        Err(DeviceError::PermissionDenied(_)) => LiveResult::Denied,
        Err(DeviceError::Unsupported(_)) => LiveResult::Unsupported,
        Err(DeviceError::InvalidInput(_)) => LiveResult::OutOfRange,
        Err(_) => LiveResult::Failed("OS のライブ表示の操作に失敗しました".to_string()),
    }
}

/// OS 側との橋渡し（全 OS で同じ経路を通す、pull 型、`RequestSlot` の facade）。
pub(crate) mod bridge {
    use super::*;

    pub(crate) static SLOT: RequestSlot<LiveResult> = RequestSlot::new();

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。b 層の実装は完了時に 1 回だけ呼ぶ。
    #[allow(dead_code)]
    pub(crate) fn deliver(request_id: u64, result: Result<LiveOutcome, DeviceError>) -> bool {
        SLOT.deliver(request_id, to_result(result))
    }
}

/// 画面の依頼を観測して OS へ渡し、届いた結果を画面へ反映する。`slot` は結果の置き場（本番は
/// [`bridge::SLOT`]、テストは専用の static）。
fn drive(
    state: &LiveActivityShowState,
    sender: &CompletionSender<LiveActivityShowState>,
    ui: &dyn LiveActivity,
    slot: &'static RequestSlot<LiveResult>,
) {
    let busy = matches!(slot.snapshot(), Some((_, None)));
    if !busy {
        if let Some((command, generation)) = take_command(state) {
            let _ = sender.send(Box::new(move |s: &mut LiveActivityShowState| {
                mark_command_taken(s, generation)
            }));
            let request_id = slot.begin();
            // 範囲外の値は OS へ渡さずその場で画面へ返す。
            let accepted = to_operation(command).and_then(|op| ui.submit(request_id, op));
            if let Err(e) = accepted {
                slot.deliver(request_id, to_result(Err(e)));
            }
            // 結果は次フレーム以降の poll で拾う。入力がなくても起こす。
            sender.request_repaint_after(POLL_INTERVAL);
            return;
        }
    }

    let generation = state.generation;
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut LiveActivityShowState, result| {
            // 古い世代の完了は `apply_result` が捨てる。詰まらせないよう常に消す。
            apply_result(s, generation, result);
            true
        },
    );
}

/// 画面の要求を検証済みの操作へ変換する。OS 型へ変換する前の唯一の検証点を通す。
fn to_operation(command: LiveCommand) -> Result<LiveOperation, DeviceError> {
    Ok(match command {
        LiveCommand::Start(v) => LiveOperation::Start(LiveValue::new(v)?),
        LiveCommand::Update(v) => LiveOperation::Update(LiveValue::new(v)?),
        LiveCommand::End => LiveOperation::End,
    })
}

/// `registry.rs` から `live_activity_show` 画面用に配線される effect ドライバ。
pub fn live_activity_show_effect(
    state: &LiveActivityShowState,
    sender: &CompletionSender<LiveActivityShowState>,
) {
    drive(
        state,
        sender,
        platform_live_activity().as_ref(),
        &bridge::SLOT,
    );
}

/// Android: Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄して以後届く旧
/// `request_id` の完了を無視させ、OS 側の表示も閉じる。再生成後の画面は `Idle` から始まるため、
/// 表示を残すと「開始」の次の操作と食い違う（契約表 1・9）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    bridge::SLOT.reset();
    platform_live_activity().dismiss_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::live_activity_show::{
        end, error_text, start, status_text, update, LivePhase,
    };
    use fandhe_device::live_activity::UnsupportedLiveActivity;
    use std::sync::Mutex;

    /// `submit` の呼び出しを記録する偽実装。`submit_error` が `Some` なら依頼自体を失敗させる。
    struct Fake {
        calls: Mutex<Vec<(u64, LiveOperation)>>,
        submit_error: Option<fn() -> DeviceError>,
    }

    impl Fake {
        fn new(submit_error: Option<fn() -> DeviceError>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                submit_error,
            }
        }
        fn count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl LiveActivity for Fake {
        fn submit(&self, request_id: u64, op: LiveOperation) -> Result<(), DeviceError> {
            self.calls.lock().unwrap().push((request_id, op));
            match self.submit_error {
                Some(f) => Err(f()),
                None => Ok(()),
            }
        }
        fn dismiss_all(&self) {}
    }

    type St = LiveActivityShowState;

    fn channel() -> (
        CompletionSender<St>,
        std::sync::mpsc::Receiver<crate::app::Completion<St>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 1 回の `drive` で送られた完了を全部状態へ適用する。
    fn pump(
        state: &mut St,
        ui: &Fake,
        slot: &'static RequestSlot<LiveResult>,
        tx: &CompletionSender<St>,
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<St>>,
    ) {
        let snapshot = state.clone();
        drive(&snapshot, tx, ui, slot);
        while let Ok(c) = rx.try_recv() {
            c(state);
        }
    }

    static S_NOREQ: RequestSlot<LiveResult> = RequestSlot::new();
    static S_FLOW: RequestSlot<LiveResult> = RequestSlot::new();
    static S_RANGE: RequestSlot<LiveResult> = RequestSlot::new();
    static S_DENIED: RequestSlot<LiveResult> = RequestSlot::new();
    static S_STALE: RequestSlot<LiveResult> = RequestSlot::new();
    static S_BUSY: RequestSlot<LiveResult> = RequestSlot::new();

    #[test]
    fn no_request_does_not_call_os() {
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        pump(&mut st, &ui, &S_NOREQ, &tx, &rx);
        assert_eq!(ui.count(), 0);
        assert_eq!(st, St::default());
    }

    #[test]
    fn flow_start_update_dismissed() {
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &ui, &S_FLOW, &tx, &rx);
        assert_eq!(ui.count(), 1);
        assert_eq!(st.phase, LivePhase::Starting);
        let (id, _) = S_FLOW.snapshot().unwrap();
        assert!(S_FLOW.deliver(id, LiveResult::Started));
        pump(&mut st, &ui, &S_FLOW, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Active { value: 0 });
        assert!(S_FLOW.snapshot().is_none());

        update(&mut st);
        pump(&mut st, &ui, &S_FLOW, &tx, &rx);
        let (id, _) = S_FLOW.snapshot().unwrap();
        // 閉じられていた: 失敗ではなく正常終了。
        assert!(bridge_deliver(&S_FLOW, id, Ok(LiveOutcome::Dismissed)));
        pump(&mut st, &ui, &S_FLOW, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Idle);
        assert_eq!(error_text(&st), "");
        assert!(!status_text(&st).is_empty());
    }

    fn bridge_deliver(
        slot: &RequestSlot<LiveResult>,
        id: u64,
        r: Result<LiveOutcome, DeviceError>,
    ) -> bool {
        slot.deliver(id, to_result(r))
    }

    #[test]
    fn out_of_range_does_not_call_os() {
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        st.phase = LivePhase::Active { value: 9999 };
        update(&mut st);
        pump(&mut st, &ui, &S_RANGE, &tx, &rx);
        assert_eq!(ui.count(), 0);
        pump(&mut st, &ui, &S_RANGE, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Active { value: 9999 });
        assert!(!error_text(&st).is_empty());
    }

    #[test]
    fn permission_denied_is_error_not_success() {
        let ui = Fake::new(Some(|| DeviceError::PermissionDenied("live_activity")));
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &ui, &S_DENIED, &tx, &rx);
        pump(&mut st, &ui, &S_DENIED, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Idle);
        assert!(!error_text(&st).is_empty());
        assert_eq!(status_text(&st), "");
    }

    #[test]
    fn unsupported_live_activity_never_succeeds_through_driver() {
        static S_UNSUP: RequestSlot<LiveResult> = RequestSlot::new();
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        let snapshot = st.clone();
        drive(&snapshot, &tx, &UnsupportedLiveActivity, &S_UNSUP);
        while let Ok(c) = rx.try_recv() {
            c(&mut st);
        }
        pump_poll(&mut st, &tx, &rx, &S_UNSUP);
        assert_eq!(st.phase, LivePhase::Idle);
        assert!(!error_text(&st).is_empty());
    }

    fn pump_poll(
        st: &mut St,
        tx: &CompletionSender<St>,
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<St>>,
        slot: &'static RequestSlot<LiveResult>,
    ) {
        let snapshot = st.clone();
        drive(&snapshot, tx, &UnsupportedLiveActivity, slot);
        while let Ok(c) = rx.try_recv() {
            c(st);
        }
    }

    #[test]
    fn stale_request_id_and_duplicate_delivery_are_dropped() {
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &ui, &S_STALE, &tx, &rx);
        let (id, _) = S_STALE.snapshot().unwrap();
        assert!(!S_STALE.deliver(id + 1, LiveResult::Started));
        assert!(S_STALE.deliver(id, LiveResult::Failed("x".into())));
        assert!(!S_STALE.deliver(id, LiveResult::Started));
        pump(&mut st, &ui, &S_STALE, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Idle);
        assert!(!error_text(&st).is_empty());
    }

    #[test]
    fn request_during_pending_is_not_submitted_twice() {
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &ui, &S_BUSY, &tx, &rx);
        assert_eq!(ui.count(), 1);
        // 結果未着のまま、未発送の要求が再び見えても渡さない。
        st.command = Some(LiveCommand::Start(0));
        pump(&mut st, &ui, &S_BUSY, &tx, &rx);
        pump(&mut st, &ui, &S_BUSY, &tx, &rx);
        assert_eq!(ui.count(), 1);
    }

    #[test]
    fn to_result_covers_all_paths() {
        assert_eq!(to_result(Ok(LiveOutcome::Started)), LiveResult::Started);
        assert_eq!(to_result(Ok(LiveOutcome::Updated)), LiveResult::Updated);
        assert_eq!(to_result(Ok(LiveOutcome::Ended)), LiveResult::Ended);
        assert_eq!(to_result(Ok(LiveOutcome::Dismissed)), LiveResult::Dismissed);
        assert_eq!(
            to_result(Err(DeviceError::PermissionDenied("live_activity"))),
            LiveResult::Denied
        );
        assert_eq!(
            to_result(Err(DeviceError::Unsupported("live_activity"))),
            LiveResult::Unsupported
        );
        assert_eq!(
            to_result(Err(DeviceError::InvalidInput("live_activity value"))),
            LiveResult::OutOfRange
        );
        for e in [
            DeviceError::Internal("live_activity start"),
            DeviceError::Cancelled("live_activity"),
        ] {
            assert!(matches!(to_result(Err(e)), LiveResult::Failed(_)));
        }
    }

    #[test]
    fn end_is_submitted_as_end() {
        static S_END: RequestSlot<LiveResult> = RequestSlot::new();
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St {
            phase: LivePhase::Active { value: 3 },
            ..St::default()
        };
        end(&mut st);
        pump(&mut st, &ui, &S_END, &tx, &rx);
        assert!(matches!(ui.calls.lock().unwrap()[0].1, LiveOperation::End));
    }
}
