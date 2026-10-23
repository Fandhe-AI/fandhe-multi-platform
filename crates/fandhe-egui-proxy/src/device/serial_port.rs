//! Phase 11 の拡充ループ（親 #131、Issue #670）型サンプル「シリアルポート」の effect ドライバ。
//! `fandhe_core::screens::serial_port_show`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::serial_port`（固定設定・3 秒の期限・`round_trip`）。
//!
//! # 専用ワーカー 1 本
//!
//! ポートの一覧も往復も、1 本の専用スレッド（`mpsc` キュー）へ順に流す（`device-bridge-contracts.md`
//! 11 節）。経路ごとに別のロックを足さない。往復の期限は**要求の受付時刻**（[`drive`] が要求を観測した
//! 時点）を起点に単調時計（`Instant`）で測るため、キューで待った時間も 3 秒に含まれる。
//! 完了は、ポートを閉じ終えてから送る（結果を届ける前に資源を確定させる、同 2・9 節）。古い世代の
//! 完了は画面側の `finish_*` が照合して捨てる。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_serial_ports`] の cfg 分岐が最終形で、各 OS は `implementation()` を持つモジュールだけを
//! 埋める（macOS: `macos_serial_port`（#670）、Windows: `windows_serial_port`（#671 が埋める。現状は
//! `Unsupported` のスタブ））。それ以外（iOS・Android・Linux）は `UnsupportedSerialPorts` 固定で、
//! ダミーの成功実装へは倒さない。Web は `fandhe-web-dom` が別経路で実装する（#672）ため、
//! ネイティブに `WebPick` が来ても OS を呼ばず `Unsupported` で完了する。
//!
//! # 終了
//!
//! [`shutdown`] を `device::shutdown` から呼ぶ。停止フラグを立て、進行中の往復が手を離すのを
//! [`SHUTDOWN_WAIT`] だけ待つ（`POLL_STEP` の 3 回分。往復は `POLL_STEP` ごとに停止を見る）。
//! 待ち切れなくてもプロセス終了で OS が fd を閉じる（守らない範囲、`fandhe_device::serial_port`）。
//! main thread でロックを握ったまま待たない。
//!
//! ポート名・送受信の中身・OS のエラー値はログにも失敗文言にも出さない（security.md A09）。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::serial_port_show::{
    finish_list, finish_round_trip, mark_running, take_request, PortFailure, RequestKind,
    SerialPortShowState,
};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use fandhe_device::serial_port::UnsupportedSerialPorts;
use fandhe_device::serial_port::{round_trip, SerialPortFailure, SerialPorts};

use crate::app::CompletionSender;

/// 終了時に進行中の往復が手を離すのを待つ上限。`POLL_STEP`（100 ms）の 3 回分。
const SHUTDOWN_WAIT: Duration = Duration::from_millis(300);

/// 現在のプラットフォームに応じた `SerialPorts` 実装を返す（プロセスで 1 個）。
fn platform_serial_ports() -> Arc<dyn SerialPorts> {
    static INSTANCE: OnceLock<Arc<dyn SerialPorts>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                super::macos_serial_port::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_serial_port::implementation()
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                Arc::new(UnsupportedSerialPorts)
            }
        })
        .clone()
}

/// `SerialPortFailure` を画面の失敗種別へ写す。`Internal` は固定文言へ畳み、OS 固有の値を
/// 画面へ漏らさない（security.md A09）。
fn to_failure(f: SerialPortFailure) -> PortFailure {
    match f {
        SerialPortFailure::Timeout => PortFailure::Timeout,
        SerialPortFailure::Busy => PortFailure::Busy,
        SerialPortFailure::Gone => PortFailure::Gone,
        SerialPortFailure::PermissionDenied => PortFailure::PermissionDenied,
        SerialPortFailure::Unsupported => PortFailure::Unsupported,
        SerialPortFailure::LineTooLong => PortFailure::LineTooLong,
        SerialPortFailure::Cancelled => PortFailure::Cancelled,
        SerialPortFailure::Internal => PortFailure::Failed("シリアルポートの操作".to_string()),
    }
}

/// ワーカーへ流す仕事。完了の送り先を持つ。
enum Job {
    List {
        generation: u64,
        sender: CompletionSender<SerialPortShowState>,
    },
    RoundTrip {
        generation: u64,
        port: String,
        /// 期限の起点（要求の受付時刻）。
        accepted_at: Instant,
        sender: CompletionSender<SerialPortShowState>,
    },
}

/// 専用ワーカー 1 本の入口。`stop` と `busy` はワーカーごとに持つ（終了処理が見る）。
struct Worker {
    tx: Sender<Job>,
    stop: Arc<AtomicBool>,
    busy: Arc<AtomicUsize>,
}

impl Worker {
    /// スレッドを起こす。起こせなければ `None`（呼び出し側が即座に失敗で完了させる）。
    fn spawn(ports: Arc<dyn SerialPorts>) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let stop = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicUsize::new(0));
        let (thread_stop, thread_busy) = (stop.clone(), busy.clone());
        std::thread::Builder::new()
            .name("serial-port".to_string())
            .spawn(move || {
                // 仕事は届いた順に 1 件ずつ処理する（一覧と往復を並行させない）。
                while let Ok(job) = rx.recv() {
                    run_job(ports.as_ref(), job, &thread_stop);
                    thread_busy.fetch_sub(1, Ordering::SeqCst);
                }
            })
            .ok()?;
        Some(Self { tx, stop, busy })
    }

    /// 仕事を積む。ワーカーが落ちていれば `false`（積めなかった仕事は返さず捨てる。
    /// 呼び出し側が別途失敗で完了させる）。
    fn submit(&self, job: Job) -> bool {
        self.busy.fetch_add(1, Ordering::SeqCst);
        if self.tx.send(job).is_err() {
            self.busy.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
        true
    }

    /// 停止を要求し、進行中の仕事が手を離すのを `wait` まで待つ。
    fn stop_and_wait(&self, wait: Duration) {
        self.stop.store(true, Ordering::SeqCst);
        let started = Instant::now();
        while self.busy.load(Ordering::SeqCst) > 0 && started.elapsed() < wait {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// 仕事 1 件を処理して完了を送る。往復はポートを閉じ終えてから結果を送る。
fn run_job(ports: &dyn SerialPorts, job: Job, stop: &AtomicBool) {
    match job {
        Job::List { generation, sender } => {
            let result = ports
                .list()
                .map(|entries| entries.into_iter().map(|e| e.name).collect::<Vec<_>>())
                .map_err(to_failure);
            // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
            let _ = sender.send(Box::new(move |s: &mut SerialPortShowState| {
                finish_list(s, generation, result);
            }));
        }
        Job::RoundTrip {
            generation,
            port,
            accepted_at,
            sender,
        } => {
            let result = round_trip(ports, &port, &|| accepted_at.elapsed(), stop)
                .map(|line| line.text)
                .map_err(to_failure);
            let _ = sender.send(Box::new(move |s: &mut SerialPortShowState| {
                finish_round_trip(s, generation, result);
            }));
        }
    }
}

fn failed_list(
    generation: u64,
    failure: PortFailure,
) -> Box<dyn FnOnce(&mut SerialPortShowState) + Send> {
    Box::new(move |s| finish_list(s, generation, Err(failure)))
}

fn failed_round_trip(
    generation: u64,
    failure: PortFailure,
) -> Box<dyn FnOnce(&mut SerialPortShowState) + Send> {
    Box::new(move |s| finish_round_trip(s, generation, Err(failure)))
}

/// 要求 1 件を処理して完了を送る（テストから任意のワーカーを差し込めるよう切り出す）。
/// `worker` が `None`（スレッドを起こせなかった）ときは要求を残さず失敗で完了する。
fn drive_with(
    state: &SerialPortShowState,
    sender: &CompletionSender<SerialPortShowState>,
    worker: Option<&Worker>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    // 期限の起点。キューで待つ時間も含めて数える。
    let accepted_at = Instant::now();
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重にワーカーへ渡さない。
    let _ = sender.send(Box::new(mark_running));

    let internal = || PortFailure::Failed("シリアルポートの操作".to_string());
    let job = match request.kind {
        RequestKind::List => Job::List {
            generation,
            sender: sender.clone(),
        },
        RequestKind::RoundTrip { port } => Job::RoundTrip {
            generation,
            port,
            accepted_at,
            sender: sender.clone(),
        },
        RequestKind::WebPick => {
            // Web のダイアログ選択はネイティブに無い。OS を呼ばず非対応で完了する。
            let _ = sender.send(failed_round_trip(generation, PortFailure::Unsupported));
            return;
        }
    };
    let is_list = matches!(job, Job::List { .. });
    if !worker.is_some_and(|w| w.submit(job)) {
        let done = if is_list {
            failed_list(generation, internal())
        } else {
            failed_round_trip(generation, internal())
        };
        let _ = sender.send(done);
    }
}

/// `registry.rs` の `effect` 区画から `serial_port_show` 画面用に配線される effect ドライバ。
pub fn serial_port_show_effect(
    state: &SerialPortShowState,
    sender: &CompletionSender<SerialPortShowState>,
) {
    if take_request(state).is_none() {
        return;
    }
    drive_with(state, sender, global_worker());
}

/// プロセスで 1 本のワーカー（初回の要求で遅延起動する）。
static WORKER: OnceLock<Option<Worker>> = OnceLock::new();

fn global_worker() -> Option<&'static Worker> {
    WORKER
        .get_or_init(|| Worker::spawn(platform_serial_ports()))
        .as_ref()
}

/// アプリ終了時に往復を止める（`device::shutdown` から呼ばれる）。一度も使われていなければ
/// ワーカーを起こさず何もしない。
pub fn shutdown() {
    if let Some(Some(worker)) = WORKER.get() {
        worker.stop_and_wait(SHUTDOWN_WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::serial_port_show::{
        reload, select, start_round_trip, start_web_pick, Phase, BUSY_ERROR, TIMEOUT_ERROR,
        UNSUPPORTED_ERROR,
    };
    use fandhe_device::serial_port::{
        SerialLink, SerialPortEntry, UnsupportedSerialPorts as Unsupported,
    };
    use std::sync::Mutex;

    fn sender_with_channel() -> (
        CompletionSender<SerialPortShowState>,
        mpsc::Receiver<crate::app::Completion<SerialPortShowState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// 台本どおりに返す偽のポート群。
    struct Fake {
        list: Result<Vec<String>, SerialPortFailure>,
        reply: Mutex<Option<Result<Vec<u8>, SerialPortFailure>>>,
        opens: AtomicUsize,
        lists: AtomicUsize,
        closes: Arc<AtomicUsize>,
    }

    impl Fake {
        fn new(
            list: Result<Vec<String>, SerialPortFailure>,
            reply: Option<Result<Vec<u8>, SerialPortFailure>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                list,
                reply: Mutex::new(reply),
                opens: AtomicUsize::new(0),
                lists: AtomicUsize::new(0),
                closes: Arc::new(AtomicUsize::new(0)),
            })
        }
    }

    struct FakeLink {
        reply: Option<Result<Vec<u8>, SerialPortFailure>>,
        closes: Arc<AtomicUsize>,
    }

    impl SerialLink for FakeLink {
        fn write_some(&mut self, buf: &[u8], _: Duration) -> Result<usize, SerialPortFailure> {
            Ok(buf.len())
        }
        fn read_some(&mut self, buf: &mut [u8], w: Duration) -> Result<usize, SerialPortFailure> {
            match self.reply.take() {
                Some(Ok(data)) => {
                    buf[..data.len()].copy_from_slice(&data);
                    Ok(data.len())
                }
                Some(Err(e)) => Err(e),
                None => {
                    // 返事が無い機器。実時間を少し進めて期限へ近づける。
                    std::thread::sleep(w.min(Duration::from_millis(50)));
                    Ok(0)
                }
            }
        }
        fn close(self: Box<Self>) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl SerialPorts for Fake {
        fn list(&self) -> Result<Vec<SerialPortEntry>, SerialPortFailure> {
            self.lists.fetch_add(1, Ordering::SeqCst);
            self.list
                .clone()
                .map(|v| v.into_iter().map(|name| SerialPortEntry { name }).collect())
        }
        fn open(&self, _name: &str) -> Result<Box<dyn SerialLink>, SerialPortFailure> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeLink {
                reply: self.reply.lock().unwrap().take(),
                closes: self.closes.clone(),
            }))
        }
    }

    fn recv(
        rx: &mpsc::Receiver<crate::app::Completion<SerialPortShowState>>,
        state: &mut SerialPortShowState,
    ) {
        rx.recv_timeout(Duration::from_secs(10))
            .expect("完了が届く")(state);
    }

    /// 一覧要求を処理し、mark_running と結果を適用した状態を返す。
    fn listed(fake: Arc<dyn SerialPorts>) -> SerialPortShowState {
        let (sender, rx) = sender_with_channel();
        let worker = Worker::spawn(fake).unwrap();
        let mut state = SerialPortShowState::default();
        drive_with(&state, &sender, Some(&worker));
        recv(&rx, &mut state);
        recv(&rx, &mut state);
        state
    }

    /// 一覧済みの状態から 1 番目を選んで往復し、結果まで適用した状態を返す。
    fn round_tripped(fake: Arc<Fake>) -> SerialPortShowState {
        let (sender, rx) = sender_with_channel();
        let worker = Worker::spawn(fake).unwrap();
        let mut state = SerialPortShowState::default();
        drive_with(&state, &sender, Some(&worker));
        recv(&rx, &mut state);
        recv(&rx, &mut state);
        select(&mut state, 0);
        start_round_trip(&mut state);
        drive_with(&state, &sender, Some(&worker));
        recv(&rx, &mut state);
        recv(&rx, &mut state);
        state
    }

    fn ports() -> Result<Vec<String>, SerialPortFailure> {
        Ok(vec!["/dev/cu.a".to_string()])
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(ports(), None);
        let worker = Worker::spawn(fake.clone()).unwrap();
        let state = SerialPortShowState {
            phase: Phase::Idle,
            ..Default::default()
        };
        drive_with(&state, &sender, Some(&worker));
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        assert_eq!(fake.lists.load(Ordering::SeqCst), 0);
        assert_eq!(fake.opens.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn list_success_and_empty_are_success() {
        let s = listed(Fake::new(ports(), None));
        assert_eq!(s.ports, Some(vec!["/dev/cu.a".to_string()]));
        assert_eq!(s.error, "");
        let s = listed(Fake::new(Ok(vec![]), None));
        assert_eq!(s.ports, Some(vec![]));
        assert_eq!(s.error, "");
    }

    #[test]
    fn list_failures_map_to_errors_not_empty_success() {
        let s = listed(Fake::new(Err(SerialPortFailure::Unsupported), None));
        assert_eq!(s.error, UNSUPPORTED_ERROR);
        assert_eq!(s.ports, None);
        let s = listed(Arc::new(Unsupported));
        assert_eq!(s.error, UNSUPPORTED_ERROR);
        let s = listed(Fake::new(Err(SerialPortFailure::Internal), None));
        assert!(s.error.starts_with("失敗しました: "), "{}", s.error);
    }

    #[test]
    fn round_trip_success_shows_line_and_closes() {
        let fake = Fake::new(ports(), Some(Ok(b"pong\n".to_vec())));
        let s = round_tripped(fake.clone());
        assert_eq!(s.received.as_deref(), Some("pong"));
        assert_eq!(s.error, "");
        assert_eq!(fake.closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn round_trip_failures_map_per_kind() {
        let fake = Fake::new(ports(), Some(Err(SerialPortFailure::Busy)));
        let s = round_tripped(fake.clone());
        assert_eq!(s.error, BUSY_ERROR);
        assert_eq!(s.received, None);
        assert_eq!(fake.closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn round_trip_without_reply_times_out() {
        let fake = Fake::new(ports(), None);
        let started = Instant::now();
        let s = round_tripped(fake.clone());
        assert_eq!(s.error, TIMEOUT_ERROR);
        assert!(started.elapsed() < Duration::from_secs(8));
        assert_eq!(fake.closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn second_drive_while_running_does_not_queue_again() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(ports(), None);
        let worker = Worker::spawn(fake.clone()).unwrap();
        let mut state = SerialPortShowState::default();
        drive_with(&state, &sender, Some(&worker));
        recv(&rx, &mut state); // mark_running
        drive_with(&state, &sender, Some(&worker));
        recv(&rx, &mut state); // finish_list
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(fake.lists.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_completion_does_not_overwrite_newer_request() {
        let (sender, rx) = sender_with_channel();
        let worker = Worker::spawn(Fake::new(ports(), None)).unwrap();
        let mut state = SerialPortShowState::default();
        drive_with(&state, &sender, Some(&worker));
        recv(&rx, &mut state); // mark_running
                               // 完了が届く前に世代が進んだ状況を再現する。
        state.phase = Phase::Idle;
        reload(&mut state);
        recv(&rx, &mut state); // 古い世代の finish_list
        assert_eq!(state.generation, 2);
        assert_eq!(state.ports, None);
        assert_eq!(state.phase, Phase::Requested);
    }

    #[test]
    fn web_pick_on_native_is_unsupported_without_touching_os() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(ports(), None);
        let worker = Worker::spawn(fake.clone()).unwrap();
        let mut state = SerialPortShowState::browser_picker();
        start_web_pick(&mut state);
        drive_with(&state, &sender, Some(&worker));
        recv(&rx, &mut state);
        recv(&rx, &mut state);
        assert_eq!(state.error, UNSUPPORTED_ERROR);
        assert_eq!(state.received, None);
        assert_eq!(fake.opens.load(Ordering::SeqCst), 0);
        assert_eq!(fake.lists.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn missing_worker_fails_instead_of_leaving_request() {
        let (sender, rx) = sender_with_channel();
        let mut state = SerialPortShowState::default();
        drive_with(&state, &sender, None);
        recv(&rx, &mut state);
        recv(&rx, &mut state);
        assert_eq!(state.phase, Phase::Idle);
        assert!(state.error.starts_with("失敗しました: "));
    }

    #[test]
    fn stop_makes_in_flight_round_trip_end_quickly() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(ports(), None);
        let worker = Worker::spawn(fake.clone()).unwrap();
        let mut state = SerialPortShowState::default();
        drive_with(&state, &sender, Some(&worker));
        recv(&rx, &mut state);
        recv(&rx, &mut state);
        select(&mut state, 0);
        start_round_trip(&mut state);
        drive_with(&state, &sender, Some(&worker));
        recv(&rx, &mut state); // mark_running
                               // ワーカーが往復を始める（open 済み）のを待ってから停止する。
        let waited = Instant::now();
        while fake.opens.load(Ordering::SeqCst) == 0 && waited.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let started = Instant::now();
        worker.stop_and_wait(Duration::from_secs(2));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(worker.busy.load(Ordering::SeqCst), 0);
        recv(&rx, &mut state);
        assert!(!state.error.is_empty());
        assert_eq!(fake.closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failure_mapping_hides_internal_detail() {
        assert_eq!(
            to_failure(SerialPortFailure::Internal),
            PortFailure::Failed("シリアルポートの操作".to_string())
        );
        assert_eq!(
            to_failure(SerialPortFailure::Cancelled),
            PortFailure::Cancelled
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn unsupported_os_dispatch_is_unsupported() {
        let p = platform_serial_ports();
        assert_eq!(p.list().map(|_| ()), Err(SerialPortFailure::Unsupported));
        assert!(matches!(p.open("x"), Err(SerialPortFailure::Unsupported)));
    }
}
