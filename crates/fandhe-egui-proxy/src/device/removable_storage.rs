//! Phase 11 の拡充ループ（親 #175）型サンプル「外部ストレージの着脱」（Issue #858）の effect ドライバ。
//! `fandhe_core::screens::removable_storage_show`（端末依存ゼロ）の要求を `take_init` / `take_start` /
//! `take_refresh` で観測し、`fandhe_device::removable_storage::RemovableStorage` を呼んで、結果を
//! `CompletionSender` 経由で注入する（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は
//! `fandhe_device::removable_storage`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_removable_storage`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS: `macos_removable_storage`、Windows: `windows_removable_storage`
//! （いずれも #858）、Android: `android_removable_storage`（JNI 実装、#859））。それ以外（iOS・Linux 等）は
//! `UnsupportedRemovableStorage` 固定で、ダミーの成功実装へは倒さない。Web は本ドライバを通らない。
//! OS のコールバックは [`register`] で得た `id` を持ち、[`fire`] を呼ぶ。macOS は `extern "C"` の
//! 受け口 `fandhe_rust_removable_storage_fire` を使う。
//!
//! # 方式と並行性
//!
//! OS コールバックは出来事を受信順の列（[`ActiveSubscription::queue`]）へ積み、wake 用の空の完了を
//! 1 件だけ送る（`pending` で畳む）。取り出し・記録・OS への一覧の読み直しは UI スレッドのこの effect が
//! 行い、開始・停止・取り出しは全てこの 1 本に集約する（契約表 11）。共通コアは `fandhe-device` の型を
//! 持てず effect は `&S` しか受け取れないため、購読と記録は [`ACTIVE`] に置く。停止は guard の drop で、
//! ロックの外で行う。一覧は覚えず、出来事が届くたびに `read_volumes` で OS から読み直して丸ごと置き換える。
//!
//! エラー文言・ログに OS のコード・ボリューム名を出さない（security.md A09）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use fandhe_core::screens::removable_storage_show::{
    apply_init, apply_list, apply_snapshot, finish_start, mark_failed, take_init, take_refresh,
    take_start, EventKind, KindView, ListView, Phase, RemovableStorageShowState, StartFailure,
    StateView, SupportView, VolumeRow,
};
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
use fandhe_device::removable_storage::UnsupportedRemovableStorage;
use fandhe_device::removable_storage::{
    EventLog, EventSupport, OnSignal, RemovableStorage, Signal, SubscriptionGuard, Volume,
    VolumeEvent, VolumeKind, VolumeState,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

type Sender = CompletionSender<RemovableStorageShowState>;

/// 現在のプラットフォームに応じた `RemovableStorage` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_removable_storage() -> Arc<dyn RemovableStorage> {
    #[cfg(target_os = "windows")]
    {
        super::windows_removable_storage::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_removable_storage::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_removable_storage::implementation()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
    {
        Arc::new(UnsupportedRemovableStorage)
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
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "android",
        test
    )),
    allow(dead_code)
)]
pub(crate) fn register(cb: OnSignal) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    lock(&CALLBACKS).insert(id, cb);
    id
}

/// 登録を外す。停止の最初に呼び、以後の [`fire`] は何もしない。
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "android",
        test
    )),
    allow(dead_code)
)]
pub(crate) fn unregister(id: u64) {
    lock(&CALLBACKS).remove(&id);
}

/// 合図を送る。ロックの内側では呼ばず、登録表に無い `id`（停止後の遅れた呼び出し）は捨てる。
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "android",
        test
    )),
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
extern "C" fn fandhe_rust_removable_storage_fire(id: u64, code: i32) {
    if let Some(event) = fandhe_device::removable_storage::event_from_apple_code(code) {
        fire(id, Signal::Event(event));
    }
}

// ---- 写像 ----------------------------------------------------------------------------------

fn kind_from_device(e: VolumeEvent) -> EventKind {
    match e {
        VolumeEvent::Mounted => EventKind::Mounted,
        VolumeEvent::WillUnmount => EventKind::WillUnmount,
        VolumeEvent::Unmounted => EventKind::Unmounted,
        VolumeEvent::Other => EventKind::Other,
    }
}

fn support_view(s: EventSupport) -> SupportView {
    SupportView { events: s.events }
}

fn volume_row(v: Volume) -> VolumeRow {
    VolumeRow {
        name: v.name,
        kind: match v.kind {
            VolumeKind::Internal => KindView::Internal,
            VolumeKind::Removable => KindView::Removable,
            VolumeKind::Fixed => KindView::Fixed,
            VolumeKind::Network => KindView::Network,
            VolumeKind::Optical => KindView::Optical,
            VolumeKind::RamDisk => KindView::RamDisk,
            VolumeKind::Other => KindView::Other,
            VolumeKind::Unknown => KindView::Unknown,
        },
        state: match v.state {
            VolumeState::Mounted => StateView::Mounted,
            VolumeState::MountedReadOnly => StateView::MountedReadOnly,
            VolumeState::NoMedia => StateView::NoMedia,
            VolumeState::Checking => StateView::Checking,
            VolumeState::Ejecting => StateView::Ejecting,
            VolumeState::Unmounted => StateView::Unmounted,
            VolumeState::Removed => StateView::Removed,
            VolumeState::Unmountable => StateView::Unmountable,
            VolumeState::Unknown => StateView::Unknown,
        },
    }
}

/// OS の読み出し結果を表示用へ写す。読み取りの失敗は `ReadFailed`（0 件へ畳まない）。
fn list_view(result: Result<Vec<Volume>, DeviceError>) -> ListView {
    match result {
        Ok(v) => ListView::Listed(v.into_iter().map(volume_row).collect()),
        Err(DeviceError::Unsupported(_)) => ListView::Unsupported,
        Err(_) => ListView::ReadFailed,
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
    queue: Arc<Mutex<Vec<VolumeEvent>>>,
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
    let _ = sender.send(Box::new(|_: &mut RemovableStorageShowState| {}));
}

fn snapshot_completion(sender: &Sender, generation: u64, log: &EventLog, list: ListView) {
    let (recent, counts) = log.snapshot();
    let recent: Vec<(u64, EventKind)> = recent
        .into_iter()
        .map(|(n, e)| (n, kind_from_device(e)))
        .collect();
    let _ = sender.send(Box::new(move |s: &mut RemovableStorageShowState| {
        apply_snapshot(s, generation, recent, counts, Some(list))
    }));
}

/// 毎フレーム 1 回、UI スレッドで同期に処理する（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &RemovableStorageShowState,
    sender: &Sender,
    active: &Mutex<Option<ActiveSubscription>>,
    imp: &dyn RemovableStorage,
) {
    // 1. 照合: 画面が受信中でない / 世代が違う購読は止める。利用者の停止、`Failed` の合図、
    //    画面の再生成後の初期 state、古い世代をここ 1 か所で吸収する。
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

    // 2. 初回: OS が知らせる種類と今の一覧を読む。
    if take_init(state) {
        let support = support_view(imp.support());
        let list = list_view(imp.read_volumes());
        let _ = sender.send(Box::new(move |s: &mut RemovableStorageShowState| {
            apply_init(s, support, list)
        }));
    }

    // 3. 再取得: 要求番号つきで読み直し、古い結果は画面側が捨てる。
    if let Some(request) = take_refresh(state) {
        let list = list_view(imp.read_volumes());
        let _ = sender.send(Box::new(move |s: &mut RemovableStorageShowState| {
            apply_list(s, request, list)
        }));
    }

    // 4. 開始（完了が届くまでの再実行で二重に起こさない）。
    if let Some(generation) = take_start(state) {
        if lock(active).is_none() {
            do_start(sender, active, imp, generation);
        }
    }

    // 5. 取り出し: 列を受信順に記録し、OS から一覧を読み直す。
    let mut failed = None;
    {
        let mut g = lock(active);
        if let Some(a) = g.as_mut() {
            if a.pending.swap(false, Ordering::SeqCst) {
                let events: Vec<VolumeEvent> = std::mem::take(&mut *lock(&a.queue));
                for e in events {
                    a.log.record(e);
                }
                let list = list_view(imp.read_volumes());
                snapshot_completion(sender, a.generation, &a.log, list);
            }
            // 6. 実行中の失敗。
            if a.failed.swap(false, Ordering::SeqCst) {
                failed = g.take();
            }
        }
    }
    if let Some(a) = failed {
        let generation = a.generation;
        drop(a); // 解除はロックの外。
        let _ = sender.send(Box::new(move |s: &mut RemovableStorageShowState| {
            mark_failed(s, generation)
        }));
    }
}

fn do_start(
    sender: &Sender,
    active: &Mutex<Option<ActiveSubscription>>,
    imp: &dyn RemovableStorage,
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
            let list = list_view(imp.read_volumes());
            // 開始成功の完了と、その時点の一覧を順に送る（完了は FIFO で届く）。
            let _ = sender.send(Box::new(move |s: &mut RemovableStorageShowState| {
                finish_start(s, generation, Ok(()))
            }));
            snapshot_completion(sender, generation, &log, list);
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
            let _ = sender.send(Box::new(move |s: &mut RemovableStorageShowState| {
                finish_start(s, generation, Err(failure))
            }));
        }
    }
}

/// `registry.rs` の `effect` 区画から `removable_storage_show` 画面用に配線される effect ドライバ。
pub fn removable_storage_show_effect(state: &RemovableStorageShowState, sender: &Sender) {
    drive(
        state,
        sender,
        &ACTIVE,
        platform_removable_storage().as_ref(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::removable_storage_show::{
        refresh, start, stop, Outcome, RUNTIME_FAILED_TEXT,
    };
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
        reads: AtomicUsize,
    }
    impl Fake {
        fn new() -> Self {
            Fake {
                support: EventSupport { events: [true; 4] },
                fail: None,
                read_fails: false,
                cb: Mutex::new(None),
                drops: Arc::new(AtomicUsize::new(0)),
                starts: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
            }
        }
        fn signal(&self, s: Signal) {
            let cb = lock(&self.cb).clone().expect("start 済み");
            cb(s);
        }
    }
    impl RemovableStorage for Fake {
        fn support(&self) -> EventSupport {
            self.support
        }
        fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            match &self.fail {
                Some(DeviceError::Unsupported(n)) => Err(DeviceError::Unsupported(n)),
                Some(DeviceError::PermissionDenied(n)) => Err(DeviceError::PermissionDenied(n)),
                Some(_) => Err(DeviceError::Internal("removable_storage start")),
                None => {
                    *lock(&self.cb) = Some(on_signal);
                    Ok(Box::new(FakeGuard(self.drops.clone())))
                }
            }
        }
        fn read_volumes(&self) -> Result<Vec<Volume>, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.read_fails {
                return Err(DeviceError::Internal("removable_storage read"));
            }
            Ok(vec![Volume {
                name: Some("Disk".into()),
                kind: VolumeKind::Removable,
                state: VolumeState::Mounted,
            }])
        }
    }

    struct Env {
        active: Mutex<Option<ActiveSubscription>>,
        fake: Fake,
        state: RemovableStorageShowState,
        sender: Sender,
        rx: mpsc::Receiver<crate::app::Completion<RemovableStorageShowState>>,
    }
    impl Env {
        fn new(fake: Fake) -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                active: Mutex::new(None),
                fake,
                state: RemovableStorageShowState::default(),
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
    fn init_reads_support_and_list() {
        let mut e = Env::new(Fake::new());
        e.tick();
        assert!(e.state.support.is_some());
        assert!(matches!(&e.state.list, ListView::Listed(v) if v.len() == 1));
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
    fn unsupported_start_is_events_unavailable_not_failure() {
        let mut fake = Fake::new();
        fake.fail = Some(DeviceError::Unsupported("removable_storage"));
        let mut e = Env::new(fake);
        start(&mut e.state);
        e.tick();
        assert_eq!(e.state.outcome, Some(Outcome::EventsUnavailable));
        assert!(lock(&e.active).is_none());
    }

    #[test]
    fn permission_denied_is_start_failed_not_success() {
        let mut fake = Fake::new();
        fake.fail = Some(DeviceError::PermissionDenied("removable_storage"));
        let mut e = Env::new(fake);
        start(&mut e.state);
        e.tick();
        assert_eq!(e.state.outcome, Some(Outcome::StartFailed));
        assert_eq!(e.state.phase, Phase::Idle);
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
        e.fake.signal(Signal::Event(VolumeEvent::Mounted));
        e.fake.signal(Signal::Event(VolumeEvent::WillUnmount));
        e.fake.signal(Signal::Event(VolumeEvent::Unmounted));
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
                (3, EventKind::Unmounted),
                (2, EventKind::WillUnmount),
                (1, EventKind::Mounted)
            ]
        );
        assert_eq!(e.state.counts[EventKind::Mounted.index()], 1);
    }

    #[test]
    fn each_event_batch_rereads_the_list() {
        let mut e = Env::receiving(Fake::new());
        let before = e.fake.reads.load(Ordering::SeqCst);
        e.fake.signal(Signal::Event(VolumeEvent::Mounted));
        e.tick();
        assert_eq!(e.fake.reads.load(Ordering::SeqCst), before + 1);
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
            fandhe_core::screens::removable_storage_show::error_text(&e.state),
            RUNTIME_FAILED_TEXT
        );
    }

    #[test]
    fn read_failure_is_reported_as_read_failed() {
        let mut fake = Fake::new();
        fake.read_fails = true;
        let mut e = Env::new(fake);
        e.tick();
        assert_eq!(e.state.list, ListView::ReadFailed);
    }

    #[test]
    fn refresh_rereads_the_list() {
        let mut e = Env::new(Fake::new());
        e.tick();
        let before = e.fake.reads.load(Ordering::SeqCst);
        refresh(&mut e.state);
        e.tick();
        assert_eq!(e.fake.reads.load(Ordering::SeqCst), before + 1);
        assert_eq!(e.state.list_applied, e.state.list_request);
    }

    #[test]
    fn stale_generation_subscription_is_stopped() {
        let mut e = Env::receiving(Fake::new());
        // 画面が作り直された（Idle の初期 state）。
        e.state = RemovableStorageShowState::default();
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
