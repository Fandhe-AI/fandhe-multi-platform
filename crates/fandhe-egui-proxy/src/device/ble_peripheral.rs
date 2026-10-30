//! Phase 11（親 #164）型サンプル「BLE の周辺機器役」（Issue #778）の effect ドライバ。
//! `fandhe_core::screens::ble_peripheral`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::ble_peripheral::BlePeripheral` で OS へ公開・停止を依頼し、結果と事象を
//! `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約）。
//! 契約と守る範囲は `fandhe_device::ble_peripheral` のモジュール doc を参照。
//!
//! `start` は非ブロッキング、`stop` は `.m` の専用キューへの短い同期呼び出しなので UI スレッドで
//! 同期に呼ぶ。待ち・期限・`request_repaint_after` は持たない（事象は OS の通知 →
//! `CompletionSender::send` が UI を起こす）。**macOS は `apple_ble_peripheral`、iOS は #779 で
//! `ios_ble_peripheral.rs` から `apple_ble_peripheral` へ委譲済み**。Windows は `windows_ble_peripheral`（Issue #1101）。
//! Android は対応表で保留のため `UnsupportedBlePeripheral`（非対応表示）に入る。
//!
//! 読み取りは「世代つきのまとめ送り」にする: 未適用件数を数え、0 → 1 になったときだけ完了を
//! 1 件送る。読み取りが大量に届いても完了キューが伸びない。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use fandhe_core::screens::ble_peripheral::{
    apply_event, apply_reads, finish_start, finish_stop, mark_in_flight, take_action,
    BlePeripheralState, EndKind, PeripheralAction, PeripheralFailure, ScreenEvent, StartResult,
    StopResult, OS_CHOSEN_NAME_LABEL,
};
#[cfg(any(
    not(any(target_os = "macos", target_os = "ios", target_os = "windows")),
    test
))]
use fandhe_device::ble_peripheral::UnsupportedBlePeripheral;
use fandhe_device::ble_peripheral::{
    BlePeripheral, EndReason, OnEvent, PeripheralEvent, StartOutcome, StopOutcome, LOCAL_NAME,
    SERVICE_UUID,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在の公開の session と通知先を持つ受け口。OS 実装（`.m` のコールバック）が事象を渡す入口で、
/// どの OS でもコンパイルし Linux CI でもテストする。判定と取り出しだけをロックの内側で行い、
/// `OnEvent` はロックの外で呼ぶ（契約表 2）。
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "windows")),
    allow(dead_code)
)]
#[derive(Default)]
pub(crate) struct SessionSlot {
    inner: Mutex<Option<(u64, OnEvent)>>,
}

#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "windows")),
    allow(dead_code)
)]
impl SessionSlot {
    /// 空の受け口。
    pub(crate) const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// session と通知先を差し替える。
    pub(crate) fn set(&self, session: u64, on_event: OnEvent) {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = Some((session, on_event));
    }

    /// 空にする（`stop` の前にロックを離すために呼ぶ）。
    pub(crate) fn clear(&self) {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// session が一致するときだけ通知先へ渡す。`Ended` なら受け口を空にし、以後を捨てる。
    pub(crate) fn deliver(&self, session: u64, event: PeripheralEvent) {
        let target = {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                Some((current, on_event)) if *current == session => {
                    let on_event = Arc::clone(on_event);
                    if matches!(event, PeripheralEvent::Ended(_)) {
                        *guard = None;
                    }
                    Some(on_event)
                }
                _ => None,
            }
        };
        if let Some(on_event) = target {
            on_event(event);
        }
    }
}

/// 画面に出す広告名。Windows はローカル名を指定できず OS が端末名を載せるため、事実と違う
/// 名前を出さずに「OS が決める」旨の文言にする。
fn advertised_name() -> &'static str {
    if cfg!(target_os = "windows") {
        OS_CHOSEN_NAME_LABEL
    } else {
        LOCAL_NAME
    }
}

fn to_end_kind(reason: EndReason) -> EndKind {
    match reason {
        EndReason::BluetoothOff => EndKind::BluetoothOff,
        EndReason::PermissionDenied => EndKind::PermissionDenied,
        EndReason::Unsupported => EndKind::Unsupported,
        EndReason::Failed => EndKind::Failed,
        EndReason::Interrupted => EndKind::Interrupted,
    }
}

/// 依頼結果を画面用へ写像する。内部構造・OS のコードは出さない（security.md A09）。
fn to_start_result(
    result: Result<StartOutcome, DeviceError>,
) -> Result<StartResult, PeripheralFailure> {
    match result {
        Ok(StartOutcome::Accepted) => Ok(StartResult::Accepted),
        Err(DeviceError::Unsupported(_)) => Ok(StartResult::Unsupported),
        Err(_) => Err(PeripheralFailure),
    }
}

/// 停止結果を画面用へ写像する。
fn to_stop_result(
    result: Result<StopOutcome, DeviceError>,
) -> Result<StopResult, PeripheralFailure> {
    match result {
        Ok(StopOutcome::Stopped) => Ok(StopResult::Stopped),
        Ok(StopOutcome::NotRunning) => Ok(StopResult::NotRunning),
        Err(_) => Err(PeripheralFailure),
    }
}

/// 世代（session）つきの `OnEvent` を作る。読み取りはまとめ送りにする。
fn make_on_event(sender: CompletionSender<BlePeripheralState>, session: u64) -> OnEvent {
    let pending = Arc::new(AtomicU64::new(0));
    Arc::new(move |event| match event {
        PeripheralEvent::Read => {
            // 0 → 1 のときだけ完了を送る。適用側が swap(0) で取り切るので、その後の読み取りは再び送られる。
            if pending.fetch_add(1, Ordering::SeqCst) == 0 {
                let pending = Arc::clone(&pending);
                let _ = sender.send(Box::new(move |s: &mut BlePeripheralState| {
                    let n = pending.swap(0, Ordering::SeqCst);
                    apply_reads(s, session, n);
                }));
            }
        }
        PeripheralEvent::Advertising => {
            let _ = sender.send(Box::new(move |s: &mut BlePeripheralState| {
                apply_event(
                    s,
                    session,
                    ScreenEvent::Advertising {
                        local_name: advertised_name().to_string(),
                        service_uuid: SERVICE_UUID.to_string(),
                    },
                );
            }));
        }
        PeripheralEvent::Ended(reason) => {
            let kind = to_end_kind(reason);
            let _ = sender.send(Box::new(move |s: &mut BlePeripheralState| {
                apply_event(s, session, ScreenEvent::Ended(kind));
            }));
        }
    })
}

/// 要求を観測して処理し、結果を画面へ返す。
fn drive(
    state: &BlePeripheralState,
    sender: &CompletionSender<BlePeripheralState>,
    imp: &dyn BlePeripheral,
) {
    let Some(view) = take_action(state) else {
        return;
    };
    let generation = view.generation;
    // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい。
    let _ = sender.send(Box::new(mark_in_flight));
    match view.action {
        PeripheralAction::Start => {
            let result =
                to_start_result(imp.start(generation, make_on_event(sender.clone(), generation)));
            let _ = sender.send(Box::new(move |s: &mut BlePeripheralState| {
                finish_start(s, generation, result);
            }));
        }
        PeripheralAction::Stop => {
            let result = to_stop_result(imp.stop());
            let _ = sender.send(Box::new(move |s: &mut BlePeripheralState| {
                finish_stop(s, generation, result);
            }));
        }
    }
}

/// プロセスに 1 つの実装（`shutdown` が公開と同じ実体に届くようにする）。
fn platform_ble_peripheral() -> &'static Arc<dyn BlePeripheral> {
    static IMPL: OnceLock<Arc<dyn BlePeripheral>> = OnceLock::new();
    IMPL.get_or_init(|| {
        #[cfg(target_os = "macos")]
        return super::apple_ble_peripheral::implementation();
        #[cfg(target_os = "ios")]
        return super::ios_ble_peripheral::implementation();
        #[cfg(target_os = "windows")]
        return super::windows_ble_peripheral::implementation();
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
        return Arc::new(UnsupportedBlePeripheral);
    })
}

/// `registry.rs` から `ble_peripheral` 画面用に配線される effect ドライバ。
pub fn ble_peripheral_effect(
    state: &BlePeripheralState,
    sender: &CompletionSender<BlePeripheralState>,
) {
    if take_action(state).is_none() {
        return;
    }
    drive(state, sender, platform_ble_peripheral().as_ref());
}

/// アプリ終了時に公開を止めて資源を解放する（`device::shutdown` から呼ばれる）。
/// 終了経路なので結果は見ない。
pub(super) fn shutdown() {
    let _ = platform_ble_peripheral().stop();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::ble_peripheral as screen;
    use screen::PeripheralPhase;

    /// 呼び出しを数え、決めた結果を返し、渡された session と `OnEvent` を保持する偽実装。
    struct Fake {
        start_calls: Mutex<u32>,
        stop_calls: Mutex<u32>,
        start_result: fn() -> Result<StartOutcome, DeviceError>,
        stop_result: fn() -> Result<StopOutcome, DeviceError>,
        on_event: Mutex<Option<(u64, OnEvent)>>,
    }

    impl BlePeripheral for Fake {
        fn start(&self, session: u64, on_event: OnEvent) -> Result<StartOutcome, DeviceError> {
            *self.start_calls.lock().unwrap() += 1;
            let r = (self.start_result)();
            if r.is_ok() {
                *self.on_event.lock().unwrap() = Some((session, on_event));
            }
            r
        }
        fn stop(&self) -> Result<StopOutcome, DeviceError> {
            *self.stop_calls.lock().unwrap() += 1;
            (self.stop_result)()
        }
    }

    fn fake(
        start_result: fn() -> Result<StartOutcome, DeviceError>,
        stop_result: fn() -> Result<StopOutcome, DeviceError>,
    ) -> Fake {
        Fake {
            start_calls: Mutex::new(0),
            stop_calls: Mutex::new(0),
            start_result,
            stop_result,
            on_event: Mutex::new(None),
        }
    }

    fn ok_fake() -> Fake {
        fake(|| Ok(StartOutcome::Accepted), || Ok(StopOutcome::Stopped))
    }

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<BlePeripheralState>>;

    fn channel() -> (CompletionSender<BlePeripheralState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn drain(s: &mut BlePeripheralState, rx: &Rx) {
        while let Ok(apply) = rx.try_recv() {
            apply(s);
        }
    }

    /// 1 回ドライバを回し、届いた完了をすべて適用する。
    fn step(
        s: &mut BlePeripheralState,
        sender: &CompletionSender<BlePeripheralState>,
        rx: &Rx,
        imp: &dyn BlePeripheral,
    ) {
        drive(s, sender, imp);
        drain(s, rx);
    }

    fn emit(f: &Fake, event: PeripheralEvent) {
        let on_event = f.on_event.lock().unwrap().clone().unwrap().1;
        on_event(event);
    }

    #[test]
    fn start_dispatches_once_then_waits() {
        let f = ok_fake();
        let (sender, rx) = channel();
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &f);
        assert_eq!(s.phase, PeripheralPhase::Waiting);
        // 完了後に回しても二重に渡さない。
        step(&mut s, &sender, &rx, &f);
        assert_eq!(*f.start_calls.lock().unwrap(), 1);
    }

    #[test]
    fn events_reach_the_screen() {
        let f = ok_fake();
        let (sender, rx) = channel();
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &f);
        emit(&f, PeripheralEvent::Advertising);
        drain(&mut s, &rx);
        assert_eq!(s.phase, PeripheralPhase::Advertising);
        assert_eq!(s.local_name, LOCAL_NAME);
        emit(&f, PeripheralEvent::Ended(EndReason::PermissionDenied));
        drain(&mut s, &rx);
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.error, screen::PERMISSION_DENIED_ERROR);
    }

    #[test]
    fn event_emitted_during_start_is_applied_after_accept() {
        struct EarlyEnd;
        impl BlePeripheral for EarlyEnd {
            fn start(&self, _s: u64, on_event: OnEvent) -> Result<StartOutcome, DeviceError> {
                on_event(PeripheralEvent::Ended(EndReason::BluetoothOff));
                Ok(StartOutcome::Accepted)
            }
            fn stop(&self) -> Result<StopOutcome, DeviceError> {
                Ok(StopOutcome::NotRunning)
            }
        }
        let (sender, rx) = channel();
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &EarlyEnd);
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.error, screen::BLUETOOTH_OFF_ERROR);
    }

    #[test]
    fn advertising_then_reads_during_start_are_counted_after_accept() {
        struct EarlyReads;
        impl BlePeripheral for EarlyReads {
            fn start(&self, _s: u64, on_event: OnEvent) -> Result<StartOutcome, DeviceError> {
                on_event(PeripheralEvent::Advertising);
                for _ in 0..3 {
                    on_event(PeripheralEvent::Read);
                }
                Ok(StartOutcome::Accepted)
            }
            fn stop(&self) -> Result<StopOutcome, DeviceError> {
                Ok(StopOutcome::NotRunning)
            }
        }
        let (sender, rx) = channel();
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &EarlyReads);
        assert_eq!(s.phase, PeripheralPhase::Advertising);
        assert_eq!(s.reads, 3);
    }

    #[test]
    fn reads_are_batched_into_one_completion() {
        let f = ok_fake();
        let (sender, rx) = channel();
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &f);
        emit(&f, PeripheralEvent::Advertising);
        drain(&mut s, &rx);
        for _ in 0..5 {
            emit(&f, PeripheralEvent::Read);
        }
        let mut completions = Vec::new();
        while let Ok(apply) = rx.try_recv() {
            completions.push(apply);
        }
        assert_eq!(completions.len(), 1);
        for apply in completions {
            apply(&mut s);
        }
        assert_eq!(s.reads, 5);
        // 取り切った後の読み取りは再び 1 件送られる。
        emit(&f, PeripheralEvent::Read);
        drain(&mut s, &rx);
        assert_eq!(s.reads, 6);
    }

    #[test]
    fn stale_session_events_are_dropped() {
        let f = ok_fake();
        let (sender, rx) = channel();
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &f);
        let old = f.on_event.lock().unwrap().clone().unwrap().1;
        screen::request_stop(&mut s);
        step(&mut s, &sender, &rx, &f);
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &f);
        // 古い公開の通知先から届いた事象は新しい公開に反映されない。
        old(PeripheralEvent::Advertising);
        drain(&mut s, &rx);
        assert_eq!(s.phase, PeripheralPhase::Waiting);
        emit(&f, PeripheralEvent::Advertising);
        drain(&mut s, &rx);
        assert_eq!(s.phase, PeripheralPhase::Advertising);
    }

    #[test]
    fn stop_calls_stop_once_and_reports_stopped() {
        let f = ok_fake();
        let (sender, rx) = channel();
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &f);
        screen::request_stop(&mut s);
        step(&mut s, &sender, &rx, &f);
        step(&mut s, &sender, &rx, &f);
        assert_eq!(*f.stop_calls.lock().unwrap(), 1);
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.notice, screen::STOPPED_NOTICE);
    }

    #[test]
    fn unsupported_and_failure_never_advertise() {
        let (sender, rx) = channel();
        let f = fake(
            || Err(DeviceError::Unsupported("ble_peripheral")),
            || Ok(StopOutcome::NotRunning),
        );
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &f);
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.notice, screen::UNSUPPORTED_NOTICE);

        let f = fake(
            || Err(DeviceError::Internal("ble_peripheral start")),
            || Ok(StopOutcome::NotRunning),
        );
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &f);
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.error, screen::START_FAILED_ERROR);
    }

    #[test]
    fn unsupported_implementation_reports_unsupported() {
        let (sender, rx) = channel();
        let mut s = BlePeripheralState::default();
        screen::request_start(&mut s);
        step(&mut s, &sender, &rx, &UnsupportedBlePeripheral);
        assert_eq!(s.phase, PeripheralPhase::Idle);
        assert_eq!(s.notice, screen::UNSUPPORTED_NOTICE);
    }

    #[test]
    fn session_slot_drops_stale_cleared_and_after_end() {
        let slot = SessionSlot::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        slot.set(
            7,
            Arc::new(move |e| {
                sink.lock().unwrap().push(e);
            }),
        );
        slot.deliver(6, PeripheralEvent::Advertising);
        assert!(log.lock().unwrap().is_empty());
        slot.deliver(7, PeripheralEvent::Advertising);
        slot.deliver(7, PeripheralEvent::Ended(EndReason::Failed));
        // Ended で空になり、以後は捨てる。
        slot.deliver(7, PeripheralEvent::Read);
        assert_eq!(log.lock().unwrap().len(), 2);

        let sink = Arc::clone(&log);
        slot.set(
            8,
            Arc::new(move |e| {
                sink.lock().unwrap().push(e);
            }),
        );
        slot.clear();
        slot.deliver(8, PeripheralEvent::Advertising);
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[test]
    fn advertised_name_is_local_name_off_windows() {
        if cfg!(target_os = "windows") {
            assert_eq!(advertised_name(), OS_CHOSEN_NAME_LABEL);
        } else {
            assert_eq!(advertised_name(), LOCAL_NAME);
        }
    }

    #[test]
    fn shutdown_reaches_shared_impl_without_panic() {
        // 共有実装に届くこと自体（パニックしない）だけを確かめる。実際の停止は #776 の目視で扱う。
        shutdown();
    }
}
