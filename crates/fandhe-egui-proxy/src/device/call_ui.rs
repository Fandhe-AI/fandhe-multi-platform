//! Phase 11 の拡充ループ（親 #87）型サンプル「通話 UI 統合」（Issue #276）の effect ドライバ。
//! `fandhe_core::screens::call_ui_show`（端末依存ゼロ）の要求を `take_command` で観測し、
//! `fandhe_device::call_ui::CallUi` で疑似着信を出し / 取り消し、届いた通知を `CompletionSender` 経由で
//! 注入する（`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::call_ui`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_call_ui`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（iOS: `ios_call_ui`（#277）、Android: `android_call_ui`（#278。再生成時は
//! `reset_pending_tracking` を `android_main` が呼ぶ））。それ以外は `UnsupportedCallUi` 固定で、
//! ダミーの成功実装へは倒さない。
//!
//! # 並行性と期限
//!
//! `call_id` は本ドライバがプロセス全体で単調増加に発番する（画面の世代は Activity 再生成で 0 に戻る
//! ため id に使わない）。call_id の照合と段階の更新は 1 つの `Mutex` の内側で行い、sink（OS スレッドから
//! 呼ばれる）も同じロックを取る。`report_incoming` / `end` は**ロックを外して**呼ぶ（実装が同期で
//! sink を呼んでもデッドロックしない）。通知は sink → `CompletionSender` の 1 経路だけで画面へ届く。
//! 期限は単調時計（`Instant`）で測り、`Reporting` / `Ringing` の間だけ `request_repaint_after` で
//! 次の判定を予約する。期限切れは状態を確定してから `end` を呼び、期限後の `Ringing` / `Answered` は
//! 成功にしない。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::call_ui_show::{
    apply_update, mark_command_taken, take_command, CallCommand, CallEnd, CallFailure,
    CallUiShowState, CallUpdate, REPORT_TIMEOUT,
};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_device::call_ui::UnsupportedCallUi;
use fandhe_device::call_ui::{
    CallUi, CallUiEvent, CallUiFailure, CallUiSink, EndReason, REPORT_DEADLINE, RING_TIMEOUT,
};

use crate::app::CompletionSender;

/// プロセスに 1 つだけ持つ実装（`end` が `report_incoming` と同じ実体に届くようにする）。
fn platform_call_ui() -> &'static Arc<dyn CallUi> {
    static UI: OnceLock<Arc<dyn CallUi>> = OnceLock::new();
    UI.get_or_init(|| {
        #[cfg(target_os = "ios")]
        {
            super::ios_call_ui::implementation()
        }
        #[cfg(target_os = "android")]
        {
            super::android_call_ui::implementation()
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Arc::new(UnsupportedCallUi)
        }
    })
}

/// 通話の段階。`Instant` は段階に入った時刻（期限の起点）。
#[derive(Clone, Copy)]
enum Stage {
    Reporting(Instant),
    Ringing(Instant),
    Active,
}

/// 進行中の通話 1 件。
struct Slot {
    call_id: u64,
    /// 要求時の画面の世代。画面へ返す通知に付ける。
    generation: u64,
    stage: Stage,
}

#[derive(Default)]
struct Inner {
    /// 処理済みの要求世代（`mark_command_taken` の反映前に二重処理しない）。
    handled: u64,
    current: Option<Slot>,
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
#[derive(Default)]
struct Session {
    inner: Arc<Mutex<Inner>>,
}

static SESSION: OnceLock<Session> = OnceLock::new();
static NEXT_CALL_ID: AtomicU64 = AtomicU64::new(1);

fn lock(m: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Activity 再生成時（`android_main` の再実行）に、ドライバの記録を捨てて進行中の通話を終わらせる。
/// 画面状態は再生成で generation 0 に戻るため、残すと次の「着信を模擬する」が処理済みとして
/// 飲み込まれる。OS 側の着信も残さない（契約表 9）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    let old = SESSION.get().and_then(|s| {
        let mut g = lock(&s.inner);
        g.handled = 0;
        g.current.take()
    });
    if let Some(slot) = old {
        platform_call_ui().end(slot.call_id);
    }
}

fn to_failure(f: CallUiFailure) -> CallFailure {
    match f {
        CallUiFailure::Unsupported => CallFailure::Unsupported,
        CallUiFailure::PermissionDenied => CallFailure::PermissionDenied,
        CallUiFailure::Disallowed => CallFailure::Disallowed,
        // OS の生コードは画面へ出さない（security.md A09）。
        CallUiFailure::Failed => CallFailure::Failed("着信の表示".to_string()),
    }
}

fn to_end(r: EndReason) -> CallEnd {
    match r {
        EndReason::Declined => CallEnd::Declined,
        EndReason::RemoteEnded => CallEnd::RemoteEnded,
        EndReason::LocalEnded => CallEnd::LocalEnded,
        EndReason::Unanswered => CallEnd::Unanswered,
    }
}

fn send_update(
    sender: &CompletionSender<CallUiShowState>,
    generation: u64,
    update: CallUpdate,
) -> bool {
    sender
        .send(Box::new(move |s: &mut CallUiShowState| {
            apply_update(s, generation, update);
        }))
        .is_ok()
}

/// call_id 照合つきの sink を作る。段階に合わない通知・期限後の `Ringing` / `Answered` は反映しない。
fn make_sink(
    inner: Arc<Mutex<Inner>>,
    sender: CompletionSender<CallUiShowState>,
    call_id: u64,
    report_deadline: Duration,
    ring_timeout: Duration,
) -> CallUiSink {
    Arc::new(move |ev: CallUiEvent| -> bool {
        let mut g = lock(&inner);
        let Some(slot) = g.current.as_mut().filter(|s| s.call_id == call_id) else {
            return true;
        };
        let generation = slot.generation;
        match (ev, slot.stage) {
            (CallUiEvent::Ringing { call_id: id }, Stage::Reporting(t))
                if id == call_id && t.elapsed() < report_deadline =>
            {
                slot.stage = Stage::Ringing(Instant::now());
                send_update(&sender, generation, CallUpdate::Ringing)
            }
            (CallUiEvent::Answered { call_id: id }, Stage::Ringing(t))
                if id == call_id && t.elapsed() < ring_timeout =>
            {
                slot.stage = Stage::Active;
                send_update(&sender, generation, CallUpdate::Answered)
            }
            (
                CallUiEvent::Ended {
                    call_id: id,
                    reason,
                },
                Stage::Ringing(_) | Stage::Active,
            ) if id == call_id => {
                g.current = None;
                send_update(&sender, generation, CallUpdate::Ended(to_end(reason)))
            }
            (
                CallUiEvent::Failed {
                    call_id: id,
                    failure,
                },
                _,
            ) if id == call_id => {
                g.current = None;
                send_update(&sender, generation, CallUpdate::Failed(to_failure(failure)))
            }
            // 段階に合わない通知・期限後の通知は捨てる（期限切れは drive が確定する）。
            _ => true,
        }
    })
}

/// 1 フレームぶんの処理。要求の発送と期限判定を行い、OS 呼び出しはロックの外で行う。
fn drive(
    state: &CallUiShowState,
    sender: &CompletionSender<CallUiShowState>,
    ui: &Arc<dyn CallUi>,
    session: &Session,
    report_deadline: Duration,
    ring_timeout: Duration,
) {
    let mut to_end: Vec<u64> = Vec::new();
    let mut to_report: Option<(u64, CallUiSink)> = None;

    {
        let mut g = lock(&session.inner);
        if let Some((command, generation)) = take_command(state) {
            if g.handled != generation {
                g.handled = generation;
                let _ = sender.send(Box::new(move |s: &mut CallUiShowState| {
                    mark_command_taken(s, generation)
                }));
                // 残っている通話は先に終わらせる（OS 側の着信を残さない）。
                if let Some(old) = g.current.take() {
                    to_end.push(old.call_id);
                }
                if command == CallCommand::Report {
                    let call_id = NEXT_CALL_ID.fetch_add(1, Ordering::SeqCst);
                    g.current = Some(Slot {
                        call_id,
                        generation,
                        stage: Stage::Reporting(Instant::now()),
                    });
                    let sink = make_sink(
                        session.inner.clone(),
                        sender.clone(),
                        call_id,
                        report_deadline,
                        ring_timeout,
                    );
                    to_report = Some((call_id, sink));
                }
            }
        }
        // 期限判定。確定（current = None と画面への通知）してから end を呼ぶ。
        if to_report.is_none() {
            let verdict = g.current.as_ref().and_then(|slot| match slot.stage {
                Stage::Reporting(t) => Some((t.elapsed(), report_deadline, true)),
                Stage::Ringing(t) => Some((t.elapsed(), ring_timeout, false)),
                Stage::Active => None,
            });
            if let Some((elapsed, limit, reporting)) = verdict {
                if elapsed >= limit {
                    if let Some(slot) = g.current.take() {
                        let update = if reporting {
                            CallUpdate::Failed(CallFailure::Failed(REPORT_TIMEOUT.to_string()))
                        } else {
                            CallUpdate::Ended(CallEnd::Unanswered)
                        };
                        send_update(sender, slot.generation, update);
                        to_end.push(slot.call_id);
                    }
                } else {
                    sender.request_repaint_after(limit - elapsed);
                }
            }
        }
    }

    for id in to_end {
        ui.end(id);
    }
    if let Some((call_id, sink)) = to_report {
        if let Err(f) = ui.report_incoming(call_id, sink) {
            let mut g = lock(&session.inner);
            if g.current.as_ref().is_some_and(|s| s.call_id == call_id) {
                if let Some(slot) = g.current.take() {
                    send_update(sender, slot.generation, CallUpdate::Failed(to_failure(f)));
                }
            }
        }
    }
}

/// `registry.rs` の `effect` 区画から `call_ui_show` 画面用に配線される effect ドライバ。
pub fn call_ui_show_effect(state: &CallUiShowState, sender: &CompletionSender<CallUiShowState>) {
    drive(
        state,
        sender,
        platform_call_ui(),
        SESSION.get_or_init(Session::default),
        REPORT_DEADLINE,
        RING_TIMEOUT,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::call_ui_show::{end_call, report, CallPhase};
    use std::sync::mpsc;

    const LONG: Duration = Duration::from_secs(60);

    struct Fake {
        result: Mutex<Option<CallUiFailure>>,
        /// `report_incoming` の中で同期に送る通知。
        sync_ringing: bool,
        sink: Mutex<Option<(u64, CallUiSink)>>,
        reports: Mutex<Vec<u64>>,
        ends: Mutex<Vec<u64>>,
    }

    impl Fake {
        fn new(result: Option<CallUiFailure>, sync_ringing: bool) -> Arc<Self> {
            Arc::new(Self {
                result: Mutex::new(result),
                sync_ringing,
                sink: Mutex::new(None),
                reports: Mutex::new(Vec::new()),
                ends: Mutex::new(Vec::new()),
            })
        }
        fn sink(&self) -> (u64, CallUiSink) {
            self.sink.lock().unwrap().clone().expect("sink")
        }
        fn send(&self, f: impl Fn(u64) -> CallUiEvent) {
            let (id, sink) = self.sink();
            sink(f(id));
        }
    }

    impl CallUi for Fake {
        fn report_incoming(&self, call_id: u64, sink: CallUiSink) -> Result<(), CallUiFailure> {
            self.reports.lock().unwrap().push(call_id);
            if let Some(f) = self.result.lock().unwrap().take() {
                return Err(f);
            }
            *self.sink.lock().unwrap() = Some((call_id, sink.clone()));
            if self.sync_ringing {
                sink(CallUiEvent::Ringing { call_id });
            }
            Ok(())
        }
        fn end(&self, call_id: u64) {
            self.ends.lock().unwrap().push(call_id);
        }
    }

    type Rx = mpsc::Receiver<crate::app::Completion<CallUiShowState>>;

    fn channel() -> (CompletionSender<CallUiShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn pump(state: &mut CallUiShowState, rx: &Rx) {
        while let Ok(c) = rx.try_recv() {
            c(state);
        }
    }

    struct H {
        fake: Arc<Fake>,
        ui: Arc<dyn CallUi>,
        session: Session,
        sender: CompletionSender<CallUiShowState>,
        rx: Rx,
        state: CallUiShowState,
    }

    impl H {
        fn new(fake: &Arc<Fake>) -> Self {
            let (sender, rx) = channel();
            Self {
                fake: fake.clone(),
                ui: fake.clone(),
                session: Session::default(),
                sender,
                rx,
                state: CallUiShowState::default(),
            }
        }
        fn drive_with(&mut self, rd: Duration, rt: Duration) {
            drive(&self.state, &self.sender, &self.ui, &self.session, rd, rt);
            pump(&mut self.state, &self.rx);
        }
        fn drive(&mut self) {
            self.drive_with(LONG, LONG);
        }
        fn report(&mut self) {
            report(&mut self.state);
            self.drive();
        }
    }

    #[test]
    fn sync_errors_are_never_success() {
        for (f, expect) in [
            (
                CallUiFailure::PermissionDenied,
                CallFailure::PermissionDenied,
            ),
            (CallUiFailure::Disallowed, CallFailure::Disallowed),
            (CallUiFailure::Unsupported, CallFailure::Unsupported),
        ] {
            let mut h = H::new(&Fake::new(Some(f), false));
            h.report();
            assert_eq!(h.state.phase, CallPhase::Failed(expect));
        }
    }

    #[test]
    fn double_drive_before_mark_applied_reports_once() {
        let fake = Fake::new(None, false);
        let mut h = H::new(&fake);
        report(&mut h.state);
        drive(&h.state, &h.sender, &h.ui, &h.session, LONG, LONG);
        drive(&h.state, &h.sender, &h.ui, &h.session, LONG, LONG);
        assert_eq!(fake.reports.lock().unwrap().len(), 1);
    }

    #[test]
    fn sync_sink_call_inside_report_does_not_deadlock() {
        let mut h = H::new(&Fake::new(None, true));
        h.report();
        assert_eq!(h.state.phase, CallPhase::Ringing);
    }

    #[test]
    fn answer_then_remote_end_reaches_screen() {
        let fake = Fake::new(None, true);
        let mut h = H::new(&fake);
        h.report();
        fake.send(|call_id| CallUiEvent::Answered { call_id });
        pump(&mut h.state, &h.rx);
        assert_eq!(h.state.phase, CallPhase::Active);
        fake.send(|call_id| CallUiEvent::Ended {
            call_id,
            reason: EndReason::RemoteEnded,
        });
        pump(&mut h.state, &h.rx);
        assert_eq!(h.state.phase, CallPhase::Ended(CallEnd::RemoteEnded));
    }

    #[test]
    fn decline_is_not_a_failure() {
        let fake = Fake::new(None, true);
        let mut h = H::new(&fake);
        h.report();
        fake.send(|call_id| CallUiEvent::Ended {
            call_id,
            reason: EndReason::Declined,
        });
        pump(&mut h.state, &h.rx);
        assert_eq!(h.state.phase, CallPhase::Ended(CallEnd::Declined));
    }

    #[test]
    fn cancel_ends_once_and_late_answer_is_dropped() {
        let fake = Fake::new(None, true);
        let mut h = H::new(&fake);
        h.report();
        let (id, _) = fake.sink();
        end_call(&mut h.state);
        h.drive();
        assert_eq!(*fake.ends.lock().unwrap(), vec![id]);
        fake.send(|call_id| CallUiEvent::Answered { call_id });
        pump(&mut h.state, &h.rx);
        assert_eq!(h.state.phase, CallPhase::Ended(CallEnd::LocalEnded));
    }

    #[test]
    fn old_call_id_does_not_leak_into_new_call() {
        let fake = Fake::new(None, true);
        let mut h = H::new(&fake);
        h.report();
        let (_, old_sink) = fake.sink();
        let old_id = fake.sink().0;
        end_call(&mut h.state);
        h.drive();
        h.report();
        assert_eq!(h.state.phase, CallPhase::Ringing);
        old_sink(CallUiEvent::Answered { call_id: old_id });
        pump(&mut h.state, &h.rx);
        assert_eq!(h.state.phase, CallPhase::Ringing);
    }

    #[test]
    fn report_deadline_fails_ends_and_rejects_late_ringing() {
        let fake = Fake::new(None, false);
        let mut h = H::new(&fake);
        report(&mut h.state);
        h.drive_with(Duration::ZERO, LONG); // 発送
        h.drive_with(Duration::ZERO, LONG); // 期限判定
        let CallPhase::Failed(CallFailure::Failed(m)) = h.state.phase.clone() else {
            panic!("{:?}", h.state.phase)
        };
        assert_eq!(m, REPORT_TIMEOUT);
        let (id, _) = fake.sink();
        assert_eq!(*fake.ends.lock().unwrap(), vec![id]);
        fake.send(|call_id| CallUiEvent::Ringing { call_id });
        pump(&mut h.state, &h.rx);
        assert!(matches!(h.state.phase, CallPhase::Failed(_)));
    }

    #[test]
    fn late_ringing_before_drive_is_not_success() {
        let fake = Fake::new(None, false);
        let mut h = H::new(&fake);
        report(&mut h.state);
        drive(&h.state, &h.sender, &h.ui, &h.session, Duration::ZERO, LONG);
        // 期限を過ぎてから、drive が確定する前に届いた Ringing は成功にしない。
        fake.send(|call_id| CallUiEvent::Ringing { call_id });
        drive(&h.state, &h.sender, &h.ui, &h.session, Duration::ZERO, LONG);
        pump(&mut h.state, &h.rx);
        assert!(matches!(h.state.phase, CallPhase::Failed(_)));
    }

    #[test]
    fn ring_timeout_is_unanswered_and_late_answer_is_dropped() {
        let fake = Fake::new(None, true);
        let mut h = H::new(&fake);
        h.report();
        h.drive_with(LONG, Duration::ZERO);
        assert_eq!(h.state.phase, CallPhase::Ended(CallEnd::Unanswered));
        assert_eq!(fake.ends.lock().unwrap().len(), 1);
        fake.send(|call_id| CallUiEvent::Answered { call_id });
        pump(&mut h.state, &h.rx);
        assert_eq!(h.state.phase, CallPhase::Ended(CallEnd::Unanswered));
    }

    #[test]
    fn reset_lets_recreated_screen_report_again() {
        let fake = Fake::new(None, true);
        let mut h = H::new(&fake);
        h.report();
        // Activity 再生成: 画面状態だけ generation 0 に戻る。
        h.state = CallUiShowState::default();
        report(&mut h.state);
        h.drive();
        assert_eq!(
            fake.reports.lock().unwrap().len(),
            1,
            "リセット無しでは飲み込まれる"
        );
        let old = {
            let mut g = lock(&h.session.inner);
            g.handled = 0;
            g.current.take()
        };
        assert!(old.is_some(), "リセット対象の通話がある");
        h.drive();
        assert_eq!(fake.reports.lock().unwrap().len(), 2);
    }

    #[test]
    fn sink_returns_false_when_receiver_dropped() {
        let fake = Fake::new(None, false);
        let mut h = H::new(&fake);
        report(&mut h.state);
        drive(&h.state, &h.sender, &h.ui, &h.session, LONG, LONG);
        let H { fake, rx, .. } = h;
        drop(rx);
        let (id, sink) = fake.sink();
        assert!(!sink(CallUiEvent::Ringing { call_id: id }));
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    #[test]
    fn unsupported_os_never_reports_success() {
        let sink: CallUiSink = Arc::new(|_| true);
        assert_eq!(
            platform_call_ui().report_incoming(1, sink),
            Err(CallUiFailure::Unsupported)
        );
    }
}
