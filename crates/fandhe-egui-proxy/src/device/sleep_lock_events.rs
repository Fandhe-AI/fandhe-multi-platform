//! Phase 11 の拡充ループ（親 #167）型サンプル「スリープ・ロックの通知」（Issue #810）の effect ドライバ。
//! `fandhe_core::screens::sleep_lock_events_show`（端末依存ゼロ）の要求を `take_init` / `take_start` で
//! 観測し、`fandhe_device::sleep_lock_events::SleepLockEvents` を呼んで、結果を `CompletionSender` 経由で
//! 注入する（`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる
//! 契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::sleep_lock_events`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_sleep_lock_events`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS: `macos_sleep_lock_events`、Windows: `windows_sleep_lock_events`
//! （いずれも #810）、Android: `android_sleep_lock_events`（#811 が埋めるスタブ））。それ以外（iOS・Linux 等）は
//! `UnsupportedSleepLockEvents` 固定で、ダミーの成功実装へは倒さない。Web は本ドライバを通らない。
//! OS のコールバックは [`register`] で得た `id` を持ち、[`fire`] を呼ぶ。macOS は `extern "C"` の
//! 受け口 `fandhe_rust_sleep_lock_events_fire` を使う。
//!
//! # 方式と並行性
//!
//! OS コールバックは出来事を受信順の列（[`ActiveSubscription::queue`]）へ積み、wake 用の空の完了を
//! 1 件だけ送る（`pending` で畳む）。取り出し・記録・OS への問い直しは UI スレッドのこの effect が行い、
//! 開始・停止・取り出しは全てこの 1 本に集約する（契約表 11）。共通コアは `fandhe-device` の型を持てず
//! effect は `&S` しか受け取れないため、購読と記録は [`ACTIVE`] に置く。停止は guard の drop で、
//! ロックの外で行う。ロック状態は覚えず、出来事が届くたびに `read_current` で OS へ問い直す。
//!
//! エラー文言・ログに OS のコード・メッセージを出さない（security.md A09）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use fandhe_core::screens::sleep_lock_events_show::{
    apply_init, apply_snapshot, finish_start, mark_failed, take_init, take_start, CurrentView,
    EventKind, LockView, Phase, ScreenView, SleepLockEventsShowState, StartFailure, SupportView,
};
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
use fandhe_device::sleep_lock_events::UnsupportedSleepLockEvents;
use fandhe_device::sleep_lock_events::{
    CurrentState, EventLog, EventSupport, LockState, OnSignal, ScreenState, Signal, SleepLockEvent,
    SleepLockEvents, SubscriptionGuard,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

type Sender = CompletionSender<SleepLockEventsShowState>;

/// 現在のプラットフォームに応じた `SleepLockEvents` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_sleep_lock_events() -> Arc<dyn SleepLockEvents> {
    #[cfg(target_os = "windows")]
    {
        super::windows_sleep_lock_events::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_sleep_lock_events::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_sleep_lock_events::implementation()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
    {
        Arc::new(UnsupportedSleepLockEvents)
    }
}

// ---- 登録表（全 OS 共通）-------------------------------------------------------------------

type Callbacks = Mutex<HashMap<u64, OnSignal>>;
static CALLBACKS: std::sync::LazyLock<Callbacks> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// OS コールバックが引く `id` を発行して合図の送り先を登録する。OS 側は生ポインタではなく
/// この `id` だけを持ち、停止後に遅れて届いた呼び出しは [`fire`] が捨てる。
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", test)),
    allow(dead_code)
)]
pub(crate) fn register(cb: OnSignal) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    lock(&CALLBACKS).insert(id, cb);
    id
}

/// 登録を外す。停止の最初に呼び、以後の [`fire`] は何もしない。
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", test)),
    allow(dead_code)
)]
pub(crate) fn unregister(id: u64) {
    lock(&CALLBACKS).remove(&id);
}

/// 合図を送る。ロックの内側では呼ばず、登録表に無い `id`（停止後の遅れた呼び出し）は捨てる。
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", test)),
    allow(dead_code)
)]
pub(crate) fn fire(id: u64, signal: Signal) {
    let cb = lock(&CALLBACKS).get(&id).cloned();
    if let Some(cb) = cb {
        cb(signal);
    }
}

/// `.m`（macOS）のコールバックから呼ばれる受け口。`code` は `event_from_apple_code` の値。
/// 未知のコードは捨てる（出来事を捏造せず、失敗にもしない）。
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_sleep_lock_events_fire(id: u64, code: i32) {
    if let Some(event) = fandhe_device::sleep_lock_events::event_from_apple_code(code) {
        fire(id, Signal::Event(event));
    }
}

// ---- 写像 ----------------------------------------------------------------------------------

fn kind_from_device(e: SleepLockEvent) -> EventKind {
    match e {
        SleepLockEvent::SystemWillSleep => EventKind::SystemWillSleep,
        SleepLockEvent::SystemDidWake => EventKind::SystemDidWake,
        SleepLockEvent::ScreenOff => EventKind::ScreenOff,
        SleepLockEvent::ScreenOn => EventKind::ScreenOn,
        SleepLockEvent::Locked => EventKind::Locked,
        SleepLockEvent::Unlocked => EventKind::Unlocked,
    }
}

fn support_view(s: EventSupport) -> SupportView {
    SupportView {
        events: s.events,
        lock: s.lock,
        screen: s.screen,
    }
}

/// OS の読み出し結果を表示用へ写す。読めない OS は `NotAvailable`、読み取りの失敗は `ReadFailed`
/// （「ロックされていない」へ畳まない）。
fn current_view(support: EventSupport, result: Result<CurrentState, DeviceError>) -> CurrentView {
    match result {
        Ok(c) => CurrentView {
            lock: match c.lock {
                Some(LockState::Locked) => LockView::Locked,
                Some(LockState::Unlocked) => LockView::Unlocked,
                Some(LockState::Unknown) => LockView::Unknown,
                None => LockView::NotAvailable,
            },
            screen: match c.screen {
                Some(ScreenState::On) => ScreenView::On,
                Some(ScreenState::Off) => ScreenView::Off,
                Some(ScreenState::Unknown) => ScreenView::Unknown,
                None => ScreenView::NotAvailable,
            },
        },
        Err(_) => CurrentView {
            lock: if support.lock {
                LockView::ReadFailed
            } else {
                LockView::NotAvailable
            },
            screen: if support.screen {
                ScreenView::ReadFailed
            } else {
                ScreenView::NotAvailable
            },
        },
    }
}

// ---- ドライバ ------------------------------------------------------------------------------

/// 実行中の購読 1 件。
struct ActiveSubscription {
    /// drop で OS の購読を解除する。
    _guard: Box<dyn SubscriptionGuard>,
    /// 開始要求の世代。
    generation: u64,
    /// OS コールバックが受信順に積む出来事の列。
    queue: Arc<Mutex<Vec<SleepLockEvent>>>,
    /// 取り出し待ちの出来事があるか（wake の畳み込み用）。
    pending: Arc<AtomicBool>,
    /// 実行中の失敗の合図。
    failed: Arc<AtomicBool>,
    /// 受信記録（UI スレッドだけが触る）。
    log: EventLog,
}

static ACTIVE: Mutex<Option<ActiveSubscription>> = Mutex::new(None);

fn wake(sender: &Sender) {
    // wake 用の空の完了。送信失敗（終了中）は無視してよい。
    let _ = sender.send(Box::new(|_: &mut SleepLockEventsShowState| {}));
}

fn snapshot_completion(sender: &Sender, generation: u64, log: &EventLog, current: CurrentView) {
    let (recent, counts) = log.snapshot();
    let recent: Vec<(u64, EventKind)> = recent
        .into_iter()
        .map(|(n, e)| (n, kind_from_device(e)))
        .collect();
    let _ = sender.send(Box::new(move |s: &mut SleepLockEventsShowState| {
        apply_snapshot(s, generation, recent, counts, Some(current))
    }));
}

/// 毎フレーム 1 回、UI スレッドで同期に処理する（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &SleepLockEventsShowState,
    sender: &Sender,
    active: &Mutex<Option<ActiveSubscription>>,
    imp: &dyn SleepLockEvents,
) {
    // 1. 照合: 画面が受信中でない / 世代が違う購読は止める。利用者の停止、`Failed` の合図、
    //    Android の Activity 再生成後の初期 state、古い世代をここ 1 か所で吸収する。
    let stale = {
        let mut g = lock(active);
        let keep = g
            .as_ref()
            .is_some_and(|a| state.phase != Phase::Idle && state.generation == a.generation);
        if keep {
            None
        } else {
            g.take()
        }
    };
    drop(stale); // 解除の待ちはロックの外。

    // 2. 初回: OS が知らせる種類と今の状態を読む。
    if take_init(state) {
        let support = imp.support();
        let current = current_view(support, imp.read_current());
        let view = support_view(support);
        let _ = sender.send(Box::new(move |s: &mut SleepLockEventsShowState| {
            apply_init(s, view, current)
        }));
    }

    // 3. 開始（完了が届くまでの再実行で二重に起こさない）。
    if let Some(generation) = take_start(state) {
        if lock(active).is_none() {
            do_start(sender, active, imp, generation);
        }
    }

    // 4. 取り出し: 列を受信順に記録し、OS へ今の状態を問い直す。
    let mut failed = None;
    {
        let mut g = lock(active);
        if let Some(a) = g.as_mut() {
            if a.pending.swap(false, Ordering::SeqCst) {
                let events: Vec<SleepLockEvent> = std::mem::take(&mut *lock(&a.queue));
                for e in events {
                    a.log.record(e);
                }
                let current = current_view(imp.support(), imp.read_current());
                snapshot_completion(sender, a.generation, &a.log, current);
            }
            // 5. 実行中の失敗。
            if a.failed.swap(false, Ordering::SeqCst) {
                failed = g.take();
            }
        }
    }
    if let Some(a) = failed {
        let generation = a.generation;
        drop(a); // 解除はロックの外。
        let _ = sender.send(Box::new(move |s: &mut SleepLockEventsShowState| {
            mark_failed(s, generation)
        }));
    }
}

fn do_start(
    sender: &Sender,
    active: &Mutex<Option<ActiveSubscription>>,
    imp: &dyn SleepLockEvents,
    generation: u64,
) {
    let queue = Arc::new(Mutex::new(Vec::new()));
    let pending = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(false));
    let cb: OnSignal = {
        let (sender, queue, pending, failed) = (
            sender.clone(),
            queue.clone(),
            pending.clone(),
            failed.clone(),
        );
        Arc::new(move |sig| match sig {
            Signal::Event(e) => {
                // 列へ積んでから合図を立てる（取り出し側が合図を見たら列に必ずある）。
                lock(&queue).push(e);
                if !pending.swap(true, Ordering::SeqCst) {
                    wake(&sender);
                }
            }
            Signal::Failed => {
                failed.store(true, Ordering::SeqCst);
                wake(&sender);
            }
        })
    };
    match imp.start(cb) {
        Ok(guard) => {
            let log = EventLog::new();
            let current = current_view(imp.support(), imp.read_current());
            snapshot_completion_after_start(sender, generation, &log, current);
            *lock(active) = Some(ActiveSubscription {
                _guard: guard,
                generation,
                queue,
                pending,
                failed,
                log,
            });
        }
        Err(e) => {
            let failure = match e {
                DeviceError::Unsupported(_) => StartFailure::Unsupported,
                _ => StartFailure::Failed,
            };
            let _ = sender.send(Box::new(move |s: &mut SleepLockEventsShowState| {
                finish_start(s, generation, Err(failure))
            }));
        }
    }
}

/// 開始成功の完了と、その時点の今の状態を順に送る（完了は FIFO で届く）。
fn snapshot_completion_after_start(
    sender: &Sender,
    generation: u64,
    log: &EventLog,
    current: CurrentView,
) {
    let _ = sender.send(Box::new(move |s: &mut SleepLockEventsShowState| {
        finish_start(s, generation, Ok(()))
    }));
    snapshot_completion(sender, generation, log, current);
}

/// `registry.rs` の `effect` 区画から `sleep_lock_events_show` 画面用に配線される effect ドライバ。
pub fn sleep_lock_events_show_effect(state: &SleepLockEventsShowState, sender: &Sender) {
    drive(
        state,
        sender,
        &ACTIVE,
        platform_sleep_lock_events().as_ref(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::sleep_lock_events_show::{start, stop, Outcome, RUNTIME_FAILED_TEXT};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    struct FakeGuard(Arc<AtomicUsize>);
    impl SubscriptionGuard for FakeGuard {}
    impl Drop for FakeGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Fake {
        support: EventSupport,
        fail: Option<DeviceError>,
        read_fails: bool,
        cb: Mutex<Option<OnSignal>>,
        drops: Arc<AtomicUsize>,
        starts: AtomicUsize,
    }
    impl Fake {
        fn new() -> Self {
            Fake {
                support: EventSupport {
                    events: [true; 6],
                    lock: true,
                    screen: false,
                },
                fail: None,
                read_fails: false,
                cb: Mutex::new(None),
                drops: Arc::new(AtomicUsize::new(0)),
                starts: AtomicUsize::new(0),
            }
        }
        fn signal(&self, s: Signal) {
            let cb = lock(&self.cb).clone().expect("start 済み");
            cb(s);
        }
    }
    impl SleepLockEvents for Fake {
        fn support(&self) -> EventSupport {
            self.support
        }
        fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            match &self.fail {
                Some(DeviceError::Unsupported(n)) => Err(DeviceError::Unsupported(n)),
                Some(_) => Err(DeviceError::Internal("sleep_lock_events start")),
                None => {
                    *lock(&self.cb) = Some(on_signal);
                    Ok(Box::new(FakeGuard(self.drops.clone())))
                }
            }
        }
        fn read_current(&self) -> Result<CurrentState, DeviceError> {
            if self.read_fails {
                return Err(DeviceError::Internal("sleep_lock_events read"));
            }
            Ok(CurrentState {
                lock: Some(LockState::Unlocked),
                screen: None,
            })
        }
    }

    struct Env {
        active: Mutex<Option<ActiveSubscription>>,
        fake: Fake,
        state: SleepLockEventsShowState,
        sender: Sender,
        rx: mpsc::Receiver<crate::app::Completion<SleepLockEventsShowState>>,
    }
    impl Env {
        fn new(fake: Fake) -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                active: Mutex::new(None),
                fake,
                state: SleepLockEventsShowState::default(),
                sender: CompletionSender::new(tx, egui::Context::default()),
                rx,
            }
        }
        fn tick(&mut self) {
            drive(&self.state, &self.sender, &self.active, &self.fake);
            while let Ok(c) = self.rx.try_recv() {
                c(&mut self.state);
            }
        }
        fn receiving(fake: Fake) -> Self {
            let mut e = Self::new(fake);
            e.tick(); // 初回の読み出し
            start(&mut e.state);
            e.tick();
            assert_eq!(e.state.phase, Phase::Receiving);
            e
        }
        fn drops(&self) -> usize {
            self.fake.drops.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn init_reads_support_and_current() {
        let mut e = Env::new(Fake::new());
        e.tick();
        assert!(e.state.support.is_some());
        assert_eq!(e.state.current.lock, LockView::Unlocked);
        assert_eq!(e.state.current.screen, ScreenView::NotAvailable);
    }

    #[test]
    fn start_failure_leaves_nothing() {
        let mut fake = Fake::new();
        fake.fail = Some(DeviceError::Internal("x"));
        let mut e = Env::new(fake);
        start(&mut e.state);
        e.tick();
        assert_eq!(e.state.phase, Phase::Idle);
        assert_eq!(e.state.outcome, Some(Outcome::StartFailed));
        assert!(lock(&e.active).is_none());
    }

    #[test]
    fn unsupported_start_is_reported_as_unsupported() {
        let mut fake = Fake::new();
        fake.fail = Some(DeviceError::Unsupported("sleep_lock_events"));
        let mut e = Env::new(fake);
        start(&mut e.state);
        e.tick();
        assert_eq!(e.state.outcome, Some(Outcome::Unsupported));
    }

    #[test]
    fn start_is_not_duplicated_before_completion() {
        let mut e = Env::new(Fake::new());
        start(&mut e.state);
        drive(&e.state, &e.sender, &e.active, &e.fake);
        drive(&e.state, &e.sender, &e.active, &e.fake);
        assert_eq!(e.fake.starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn events_are_numbered_in_receive_order_and_wake_once() {
        let mut e = Env::receiving(Fake::new());
        e.fake.signal(Signal::Event(SleepLockEvent::ScreenOff));
        e.fake.signal(Signal::Event(SleepLockEvent::ScreenOn));
        e.fake.signal(Signal::Event(SleepLockEvent::Locked));
        // wake は 1 件だけ（pending で畳む）。
        let mut wakes = 0;
        while let Ok(_c) = e.rx.try_recv() {
            wakes += 1;
        }
        assert_eq!(wakes, 1);
        e.tick();
        let kinds: Vec<_> = e.state.recent.iter().map(|(n, k)| (*n, *k)).collect();
        assert_eq!(
            kinds,
            vec![
                (3, EventKind::Locked),
                (2, EventKind::ScreenOn),
                (1, EventKind::ScreenOff)
            ]
        );
        assert_eq!(e.state.counts[EventKind::Locked.index()], 1);
    }

    #[test]
    fn stop_drops_guard_and_late_signals_are_discarded() {
        let mut e = Env::receiving(Fake::new());
        stop(&mut e.state);
        e.tick();
        assert_eq!(e.drops(), 1);
        assert!(lock(&e.active).is_none());
        e.tick();
        assert!(e.state.recent.is_empty());
    }

    #[test]
    fn failed_signal_releases_guard_and_marks_failed() {
        let mut e = Env::receiving(Fake::new());
        e.fake.signal(Signal::Failed);
        e.tick();
        assert_eq!(e.drops(), 1);
        assert_eq!(e.state.phase, Phase::Idle);
        assert_eq!(e.state.outcome, Some(Outcome::RuntimeFailed));
        assert_eq!(
            fandhe_core::screens::sleep_lock_events_show::error_text(&e.state),
            RUNTIME_FAILED_TEXT
        );
    }

    #[test]
    fn read_failure_is_reported_as_read_failed() {
        let mut fake = Fake::new();
        fake.read_fails = true;
        let mut e = Env::new(fake);
        e.tick();
        assert_eq!(e.state.current.lock, LockView::ReadFailed);
        assert_eq!(e.state.current.screen, ScreenView::NotAvailable);
    }

    #[test]
    fn stale_generation_subscription_is_stopped() {
        let mut e = Env::receiving(Fake::new());
        // 画面が作り直された（Idle の初期 state）。
        e.state = SleepLockEventsShowState::default();
        e.tick();
        assert_eq!(e.drops(), 1);
        assert!(lock(&e.active).is_none());
    }

    #[test]
    fn registry_drops_unregistered_ids() {
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let id = register(Arc::new(move |_| {
            h.fetch_add(1, Ordering::SeqCst);
        }));
        fire(id, Signal::Failed);
        unregister(id);
        fire(id, Signal::Failed);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
