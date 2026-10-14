//! Phase 11 の拡充ループ（親 #1143）型サンプル「フォルダの変更監視」（Issue #1410）の effect ドライバ。
//! `fandhe_core::screens::file_watch_show`（端末依存ゼロ）の要求を `take_start` / `take_op` で観測し、
//! `fandhe_device::file_watch::FileWatch` を呼んで、結果を `CompletionSender` 経由で注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる
//! 契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::file_watch`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_file_watch`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS: `macos_file_watch`、Windows: `windows_file_watch`（#1411）、
//! iOS: `ios_file_watch`（#1412）、Android: `android_file_watch`（#1413））。それ以外は
//! `UnsupportedFileWatch` 固定で、ダミーの成功実装へは倒さない。Web は本ドライバを通らない（#1414）。
//! OS のコールバックは [`register`] で得た `id` を持ち、[`fire`] を呼ぶ。macOS / iOS は
//! `extern "C"` の受け口 `fandhe_rust_file_watch_fire` を使う。
//!
//! # 方式と並行性
//!
//! 合図（`Changed`）は `pending` フラグに畳み、wake 用の空の完了を 1 件だけ送る（100 回届いても
//! 完了は 1 件）。記録は UI スレッドのこの effect が [`scan`] と [`diff`] で作る。開始・停止・
//! 操作・再走査は全てこの 1 本の effect に集約し（契約表 11）、OS コールバックが触るのは
//! `pending` と完了の送信だけ。共通コアは `fandhe-device` の型を持てず effect は `&S` しか受け取れない
//! ため、基準の走査結果は [`ACTIVE`] に置く。停止は guard の drop で、ロックの外で行う。
//!
//! エラー文言・ログにパス・ファイル名・OS のメッセージを出さない（security.md A09）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use fandhe_core::screens::file_watch_show::{
    apply_scan, finish_op, finish_start, mark_failed, take_op, take_start, FileWatchShowState,
    OpKind, RecordKind, StartInfo, WatchFailure, WatchPhase,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::file_watch::UnsupportedFileWatch;
use fandhe_device::file_watch::{
    create_sample, delete_sample, diff, scan, update_sample, ChangeKind, FileWatch, FolderScan,
    OnSignal, WatchGuard, WatchSignal,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 画面へ出す固定の失敗文言（OS の生コード・名前は出さない）。
const START_FAILED: &str = "監視の開始";
const OP_FAILED: &str = "ファイルの操作";

/// 作業フォルダの名前（デスクトップは `~/.fandhe/file-watch`）。
const WORK_DIR_NAME: &str = "file-watch";

/// 現在のプラットフォームに応じた `FileWatch` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_file_watch() -> Arc<dyn FileWatch> {
    #[cfg(target_os = "windows")]
    {
        super::windows_file_watch::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_file_watch::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_file_watch::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_file_watch::implementation()
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        Arc::new(UnsupportedFileWatch)
    }
}

// ---- 登録表（全 OS 共通）-------------------------------------------------------------------

// 登録表は macOS 以外では後続の子（#1411〜#1413）が使い始めるまで未使用。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
type Callbacks = Mutex<HashMap<u64, OnSignal>>;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
static CALLBACKS: std::sync::LazyLock<Callbacks> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// OS コールバックが引く `id` を発行して合図の送り先を登録する。OS 側は生ポインタではなく
/// この `id` だけを持ち、停止後に遅れて届いた呼び出しは [`fire`] が捨てる。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn register(cb: OnSignal) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    lock(&CALLBACKS).insert(id, cb);
    id
}

/// 登録を外す。停止の最初に呼び、以後の [`fire`] は何もしない。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn unregister(id: u64) {
    lock(&CALLBACKS).remove(&id);
}

/// 合図を送る。ロックの内側では呼ばず、登録表に無い `id`（停止後の遅れた呼び出し）は捨てる。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn fire(id: u64, signal: WatchSignal) {
    let cb = lock(&CALLBACKS).get(&id).cloned();
    if let Some(cb) = cb {
        cb(signal);
    }
}

/// 受け口の `kind`（0 = 変化の合図）を合図へ写す。未知の値で「動いている」を装わず `Failed`。
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
fn signal_from_code(kind: i32) -> WatchSignal {
    if kind == 0 {
        WatchSignal::Changed
    } else {
        WatchSignal::Failed
    }
}

/// `.m`（macOS / iOS）のコールバックから呼ばれる受け口。`kind` は 0 = 変化、それ以外 = 失敗。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_file_watch_fire(id: u64, kind: i32) {
    fire(id, signal_from_code(kind));
}

// ---- ドライバ ------------------------------------------------------------------------------

/// 実行中の監視 1 件。
struct ActiveWatch {
    /// drop で OS の監視を止める。
    _guard: Box<dyn WatchGuard>,
    /// 開始要求の世代。
    generation: u64,
    /// 作業フォルダ（実パス）。
    dir: PathBuf,
    /// 基準の走査結果。
    prev: FolderScan,
    /// 再走査待ちの合図があるか（まとめ送り用）。
    pending: Arc<AtomicBool>,
}

static ACTIVE: Mutex<Option<ActiveWatch>> = Mutex::new(None);

/// 作業フォルダを解決して作る。外部入力からパスを組み立てない。
fn work_dir() -> Result<PathBuf, DeviceError> {
    #[cfg(target_os = "ios")]
    let base = super::ios::documents_dir()?.0;
    #[cfg(target_os = "android")]
    let base = super::android::data_dir()
        .ok_or(DeviceError::Internal("file_watch dir"))?
        .0;
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let base = fandhe_device::storage::FsStorage::desktop_default()?.0;
    let dir = base.join(WORK_DIR_NAME);
    std::fs::create_dir_all(&dir).map_err(|_| DeviceError::Internal("file_watch dir"))?;
    Ok(dir)
}

fn to_failure(e: DeviceError, message: &str) -> WatchFailure {
    match e {
        DeviceError::Unsupported(_) => WatchFailure::Unsupported,
        _ => WatchFailure::Failed(message.to_string()),
    }
}

fn to_record_kind(k: ChangeKind) -> RecordKind {
    match k {
        ChangeKind::Created => RecordKind::Created,
        ChangeKind::Modified => RecordKind::Modified,
        ChangeKind::Deleted => RecordKind::Deleted,
    }
}

type Sender = CompletionSender<FileWatchShowState>;

/// 開始を処理する。失敗した経路では guard（登録解除 + 停止）を drop してから失敗を返す。
fn do_start(
    sender: &Sender,
    active: &Mutex<Option<ActiveWatch>>,
    fw: &dyn FileWatch,
    resolve_dir: &dyn Fn() -> Result<PathBuf, DeviceError>,
    generation: u64,
) -> Result<StartInfo, WatchFailure> {
    let fail = |e| to_failure(e, START_FAILED);
    let dir = resolve_dir().map_err(fail)?;
    let pending = Arc::new(AtomicBool::new(false));
    let cb: OnSignal = {
        let (sender, pending) = (sender.clone(), pending.clone());
        Arc::new(move |sig| match sig {
            WatchSignal::Changed => {
                if !pending.swap(true, Ordering::SeqCst) {
                    // wake 用の空の完了。送信失敗（終了中）は無視してよい。
                    let _ = sender.send(Box::new(|_: &mut FileWatchShowState| {}));
                }
            }
            WatchSignal::Failed => {
                let _ = sender.send(Box::new(move |s: &mut FileWatchShowState| {
                    mark_failed(s, generation)
                }));
            }
        })
    };
    // 登録 → OS 開始 → 基準の走査の順。走査の失敗では guard をここで drop する。
    let guard = fw.start(&dir, cb).map_err(fail)?;
    let prev = scan(&dir).map_err(fail)?;
    let info = StartInfo {
        folder: dir.display().to_string(),
        count: prev.files.len(),
        truncated: prev.truncated,
    };
    *lock(active) = Some(ActiveWatch {
        _guard: guard,
        generation,
        dir,
        prev,
        pending,
    });
    Ok(info)
}

fn do_op(dir: &std::path::Path, kind: OpKind) -> Result<bool, DeviceError> {
    match kind {
        OpKind::Create => create_sample(dir).map(|_| true),
        OpKind::Update => update_sample(dir),
        OpKind::Delete => delete_sample(dir),
    }
}

/// 毎フレーム 1 回、UI スレッドで同期に処理する（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &FileWatchShowState,
    sender: &Sender,
    active: &Mutex<Option<ActiveWatch>>,
    fw: &dyn FileWatch,
    resolve_dir: &dyn Fn() -> Result<PathBuf, DeviceError>,
) {
    // 1. 照合: 画面が監視中でない / 世代が違う監視は止める。利用者の停止、`Failed` の合図、
    //    Android の Activity 再生成後の初期 state、古い世代をここ 1 か所で吸収する。
    let stale = {
        let mut g = lock(active);
        let keep = g.as_ref().is_some_and(|w| {
            state.phase != WatchPhase::Idle && state.watch_generation == w.generation
        });
        if keep {
            None
        } else {
            g.take()
        }
    };
    drop(stale); // 停止の待ちはロックの外。

    // 2. 開始（完了が届くまでの再実行で二重に起こさない）。
    if let Some(generation) = take_start(state) {
        if lock(active).is_none() {
            let result = do_start(sender, active, fw, resolve_dir, generation);
            let _ = sender.send(Box::new(move |s: &mut FileWatchShowState| {
                finish_start(s, generation, result)
            }));
        }
    }

    // 3. 操作。変化そのものは合図 → 再走査の経路で記録に出る。
    if let Some(op) = take_op(state) {
        let dir = lock(active).as_ref().map(|w| w.dir.clone());
        let result = match dir {
            Some(d) => do_op(&d, op.kind).map_err(|e| to_failure(e, OP_FAILED)),
            None => Err(WatchFailure::Failed(OP_FAILED.to_string())),
        };
        let _ = sender.send(Box::new(move |s: &mut FileWatchShowState| {
            finish_op(s, op.generation, result)
        }));
    }

    // 4. 再走査。走査中に来た合図は `pending` が立ち直して次フレームを起こす。
    let mut failed = None;
    {
        let mut g = lock(active);
        if let Some(w) = g.as_mut() {
            if w.pending.swap(false, Ordering::SeqCst) {
                match scan(&w.dir) {
                    Ok(next) => {
                        let changes: Vec<_> = diff(&w.prev, &next)
                            .into_iter()
                            .map(|c| {
                                let name = fandhe_device::device_info::clean_text(&c.name)
                                    .unwrap_or_else(|| "?".to_string());
                                (to_record_kind(c.kind), name)
                            })
                            .collect();
                        let (generation, count, truncated) =
                            (w.generation, next.files.len(), next.truncated);
                        w.prev = next;
                        let _ = sender.send(Box::new(move |s: &mut FileWatchShowState| {
                            apply_scan(s, generation, changes, count, truncated)
                        }));
                    }
                    // フォルダ消失・走査失敗は止まった監視として失敗にする。
                    Err(_) => failed = g.take(),
                }
            }
        }
    }
    if let Some(w) = failed {
        let generation = w.generation;
        drop(w);
        let _ = sender.send(Box::new(move |s: &mut FileWatchShowState| {
            mark_failed(s, generation)
        }));
    }
}

/// `registry.rs` の `effect` 区画から `file_watch_show` 画面用に配線される effect ドライバ。
pub fn file_watch_show_effect(state: &FileWatchShowState, sender: &Sender) {
    drive(
        state,
        sender,
        &ACTIVE,
        platform_file_watch().as_ref(),
        &work_dir,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::file_watch_show::{
        create, delete, start, stop, update, FAILED_PREFIX, MONITOR_STOPPED_ERROR,
        NO_TARGET_STATUS, UNSUPPORTED_STATUS, WATCHING_STATUS,
    };
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    struct FakeGuard(Arc<AtomicUsize>);
    impl WatchGuard for FakeGuard {}
    impl Drop for FakeGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct Fake {
        fail: Option<DeviceError>,
        cb: Mutex<Option<OnSignal>>,
        drops: Arc<AtomicUsize>,
        starts: AtomicUsize,
    }
    impl Fake {
        fn signal(&self, s: WatchSignal) {
            let cb = lock(&self.cb).clone().expect("start 済み");
            cb(s);
        }
    }
    impl FileWatch for Fake {
        fn start(
            &self,
            _dir: &std::path::Path,
            on_signal: OnSignal,
        ) -> Result<Box<dyn WatchGuard>, DeviceError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            match &self.fail {
                Some(DeviceError::Unsupported(n)) => Err(DeviceError::Unsupported(n)),
                Some(_) => Err(DeviceError::Internal("file_watch start")),
                None => {
                    *lock(&self.cb) = Some(on_signal);
                    Ok(Box::new(FakeGuard(self.drops.clone())))
                }
            }
        }
    }

    struct Env {
        dir: PathBuf,
        active: Mutex<Option<ActiveWatch>>,
        fake: Fake,
        state: FileWatchShowState,
        sender: Sender,
        rx: mpsc::Receiver<crate::app::Completion<FileWatchShowState>>,
    }
    impl Env {
        fn new(tag: &str, fake: Fake) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "fandhe-fw-driver-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let (tx, rx) = mpsc::channel();
            Self {
                dir,
                active: Mutex::new(None),
                fake,
                state: FileWatchShowState::default(),
                sender: CompletionSender::new(tx, egui::Context::default()),
                rx,
            }
        }
        fn tick(&mut self) {
            let dir = self.dir.clone();
            drive(&self.state, &self.sender, &self.active, &self.fake, &|| {
                Ok(dir.clone())
            });
            while let Ok(c) = self.rx.try_recv() {
                c(&mut self.state);
            }
        }
        fn watching(tag: &str) -> Self {
            let mut e = Self::new(tag, Fake::default());
            start(&mut e.state);
            e.tick();
            assert_eq!(e.state.phase, WatchPhase::Watching);
            e
        }
        fn drops(&self) -> usize {
            self.fake.drops.load(Ordering::SeqCst)
        }
    }
    impl Drop for Env {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn start_success_reports_count() {
        let mut e = Env::new("start", Fake::default());
        std::fs::write(e.dir.join("a.txt"), "x").unwrap();
        start(&mut e.state);
        e.tick();
        assert_eq!(e.state.phase, WatchPhase::Watching);
        assert_eq!(e.state.file_count, 1);
        assert_eq!(e.state.status, WATCHING_STATUS);
        // 完了が届く前の再実行で二重に開始しない。
        e.tick();
        assert_eq!(e.fake.starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn signal_triggers_rescan_and_record() {
        let mut e = Env::watching("rescan");
        std::fs::write(e.dir.join("new.txt"), "x").unwrap();
        e.fake.signal(WatchSignal::Changed);
        e.tick();
        assert_eq!(e.state.records.len(), 1);
        assert_eq!(e.state.records[0].name, "new.txt");
        assert_eq!(e.state.records[0].kind, RecordKind::Created);
        assert_eq!(e.state.file_count, 1);
    }

    #[test]
    fn many_signals_coalesce_into_one_wake() {
        let e = Env::watching("coalesce");
        for _ in 0..100 {
            e.fake.signal(WatchSignal::Changed);
        }
        assert_eq!(e.rx.try_iter().count(), 1);
    }

    #[test]
    fn signal_during_idle_scan_is_not_lost() {
        let mut e = Env::watching("again");
        e.fake.signal(WatchSignal::Changed);
        e.tick();
        // 走査の後に届いた合図は次のフレームで再び走査を起こす。
        std::fs::write(e.dir.join("late.txt"), "x").unwrap();
        e.fake.signal(WatchSignal::Changed);
        e.tick();
        assert_eq!(e.state.records.len(), 1);
    }

    #[test]
    fn stop_drops_guard_and_is_not_failure() {
        let mut e = Env::watching("stop");
        stop(&mut e.state);
        e.tick();
        assert_eq!(e.drops(), 1);
        assert!(lock(&e.active).is_none());
        assert!(e.state.error.is_empty());
        // 停止後の合図は反映されない。
        e.fake.signal(WatchSignal::Failed);
        e.tick();
        assert!(e.state.error.is_empty());
    }

    #[test]
    fn failed_signal_marks_failed_and_releases() {
        let mut e = Env::watching("failed");
        e.fake.signal(WatchSignal::Failed);
        e.tick();
        assert_eq!(e.state.error, MONITOR_STOPPED_ERROR);
        e.tick();
        assert_eq!(e.drops(), 1);
    }

    #[test]
    fn idle_state_with_active_watch_is_stopped() {
        // Activity 再生成で state だけが初期化された想定。
        let mut e = Env::watching("recreate");
        e.state = FileWatchShowState::default();
        e.tick();
        assert_eq!(e.drops(), 1);
        assert!(lock(&e.active).is_none());
    }

    #[test]
    fn start_failure_leaves_nothing_and_is_not_success() {
        let mut e = Env::new(
            "startfail",
            Fake {
                fail: Some(DeviceError::Internal("x")),
                ..Fake::default()
            },
        );
        start(&mut e.state);
        e.tick();
        assert_eq!(e.state.phase, WatchPhase::Idle);
        assert!(e.state.error.starts_with(FAILED_PREFIX));
        assert!(lock(&e.active).is_none());
    }

    #[test]
    fn unsupported_start_shows_unsupported() {
        let mut e = Env::new(
            "unsup",
            Fake {
                fail: Some(DeviceError::Unsupported("file_watch")),
                ..Fake::default()
            },
        );
        start(&mut e.state);
        e.tick();
        assert_eq!(e.state.error, UNSUPPORTED_STATUS);
        assert_eq!(e.state.phase, WatchPhase::Idle);
    }

    #[test]
    fn baseline_scan_failure_releases_guard() {
        let mut e = Env::new("scanfail", Fake::default());
        start(&mut e.state);
        let gone = e.dir.join("missing");
        drive(
            &e.state,
            &e.sender,
            &e.active,
            &e.fake,
            &|| Ok(gone.clone()),
        );
        while let Ok(c) = e.rx.try_recv() {
            c(&mut e.state);
        }
        assert_eq!(e.drops(), 1);
        assert!(lock(&e.active).is_none());
        assert!(e.state.error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn rescan_failure_stops_with_failure() {
        let mut e = Env::watching("rescanfail");
        std::fs::remove_dir_all(&e.dir).unwrap();
        e.fake.signal(WatchSignal::Changed);
        e.tick();
        assert_eq!(e.state.error, MONITOR_STOPPED_ERROR);
        assert_eq!(e.drops(), 1);
        std::fs::create_dir_all(&e.dir).unwrap();
    }

    #[test]
    fn operations_change_folder_and_report_missing_target() {
        let mut e = Env::watching("ops");
        delete(&mut e.state);
        e.tick();
        assert_eq!(e.state.status, NO_TARGET_STATUS);
        create(&mut e.state);
        e.tick();
        assert!(e.dir.join("sample-1.txt").exists());
        e.fake.signal(WatchSignal::Changed);
        update(&mut e.state);
        e.tick();
        assert!(e.state.records.iter().any(|r| r.name == "sample-1.txt"));
    }

    #[test]
    fn registry_drops_late_calls() {
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let id = register(Arc::new(move |_| {
            h.fetch_add(1, Ordering::SeqCst);
        }));
        fire(id, WatchSignal::Changed);
        unregister(id);
        fire(id, WatchSignal::Changed);
        fire(u64::MAX, WatchSignal::Failed);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unknown_code_is_failed() {
        assert_eq!(signal_from_code(0), WatchSignal::Changed);
        assert_eq!(signal_from_code(1), WatchSignal::Failed);
        assert_eq!(signal_from_code(-7), WatchSignal::Failed);
    }

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    #[test]
    fn dispatch_without_implementation_is_unsupported() {
        let r = platform_file_watch().start(std::path::Path::new("."), Arc::new(|_| {}));
        assert!(matches!(r, Err(DeviceError::Unsupported(_))));
    }
}
