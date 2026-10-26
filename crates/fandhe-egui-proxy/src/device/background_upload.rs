//! Phase 11 の拡充ループ（親 #170、Issue #835）型サンプル「バックグラウンド送信」の effect ドライバ。
//! `fandhe_core::screens::background_upload`（端末依存ゼロ）のボタン要求を観測して
//! `fandhe_device::background_upload::BackgroundUpload` を呼び、`CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から呼ばれる契約。`device/background_download.rs` と同じ立ち位置）。
//!
//! # OS 実装の差し込み口
//!
//! [`implementation`] は全対象 OS の最終形。macOS は `apple_background_upload`（本 issue）、iOS・Android は
//! `ios_background_upload`（#836、macOS と共用実装）・`android_background_upload`（後続の子 #837 が埋めるスタブ）が返る。
//! Windows・Web は対応表で保留のため、fallback 行で `Unsupported` 固定にする。
//!
//! # OS を正にする（ポーリングしない）
//!
//! 送信の状態はアプリ内に持たない。画面を開いたとき（`needs_read`）と、OS の通知が
//! [`BackgroundUpload::set_signal`] の合図を呼んだときに、OS の一覧を `status()` で読み直して画面へ反映する。
//! 周期のタイマー（`request_repaint_after`）で状態を推測しない。合図は [`ReadGate`] の `pending` に畳み、
//! 立っていなかったときだけ wake 用の空の完了を 1 件送る（OS コールバックから egui を起こす）。
//!
//! # 並行性（契約表 2・11）
//!
//! OS 呼び出し（`start` / `cancel` / `status`）は専用スレッド 1 本の直列ワーカーに集約する。開始・取り消しの
//! 完了は `finish_action` と最新の `status()` を 1 つの closure で反映し、古い表示の 1 フレームで二重に開始できる
//! 窓を作らない。開始の前にも `status()` を読み、待機中・送信中なら `start` を呼ばない。処理中に届いた合図は
//! 捨てず `pending` に残し、完了後のフレームで読み直す。
//!
//! エラー文言・ログに OS のコード・メッセージ・URL を出さない（security.md A09）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, OnceLock};

use fandhe_core::screens::background_upload::{
    apply_snapshot, finish_action, mark_in_flight, needs_read, take_request, Action, ActionFailure,
    BackgroundUploadState, FailureView, UploadView,
};
use fandhe_device::background_upload::{BackgroundUpload, UploadFailure, UploadStatus, FILE_ERROR};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 送信先。既定はローカルの確認用の受け口（`scripts/device/serve-upload-sink.mjs`）。外部 URL は既定値に
/// 入れない。ビルド時の `FANDHE_UPLOAD_URL` で差し替えられる（`background_download` と同じ方式）。
/// 信頼の境界は実行時の `check_url`（`start` の冒頭）で守る。
const UPLOAD_URL: &str = match option_env!("FANDHE_UPLOAD_URL") {
    Some(url) => url,
    None => "http://127.0.0.1:8789/upload",
};

type Task = Box<dyn FnOnce() + Send>;

/// 読み直しの要求を畳む 2 つの旗。
#[derive(Default)]
struct ReadGate {
    /// OS の合図が届いており、まだ読み直していない。
    pending: AtomicBool,
    /// 読み出しをワーカーへ積み済みで、まだ着手していない。
    queued: AtomicBool,
}

impl ReadGate {
    /// 合図を記録する。立っていなかった（初めての合図の）ときだけ `true`（wake を送る）。
    fn mark_pending(&self) -> bool {
        !self.pending.swap(true, Ordering::SeqCst)
    }
}

static GATE: ReadGate = ReadGate {
    pending: AtomicBool::new(false),
    queued: AtomicBool::new(false),
};

/// 開始の失敗を操作の失敗理由へ写す（固定の種別。OS の生の文言を持ち込まない）。
fn start_failure(e: DeviceError) -> ActionFailure {
    match e {
        DeviceError::Unsupported(_) => ActionFailure::Unsupported,
        DeviceError::Http(_) | DeviceError::InvalidInput(_) => ActionFailure::InvalidDestination,
        DeviceError::Internal(FILE_ERROR) => ActionFailure::FileUnavailable,
        _ => ActionFailure::StartFailed,
    }
}

/// 取り消しの失敗を操作の失敗理由へ写す。
fn cancel_failure(e: DeviceError) -> ActionFailure {
    match e {
        DeviceError::Unsupported(_) => ActionFailure::Unsupported,
        _ => ActionFailure::CancelFailed,
    }
}

fn failure_view(f: UploadFailure) -> FailureView {
    match f {
        UploadFailure::Http(c) => FailureView::Http(c),
        UploadFailure::SizeMismatch { sent, expected } => {
            FailureView::SizeMismatch { sent, expected }
        }
        UploadFailure::Network => FailureView::Network,
        UploadFailure::FileUnavailable => FailureView::FileUnavailable,
        UploadFailure::RedirectNotAllowed => FailureView::RedirectNotAllowed,
        UploadFailure::Other => FailureView::Other,
    }
}

/// OS の状態を画面用へ写す。読み出しの失敗は未開始（`Idle`）に畳まず `ReadFailed`、非対応は `Unsupported`。
fn to_view(status: Result<UploadStatus, DeviceError>) -> UploadView {
    match status {
        Ok(UploadStatus::Idle) => UploadView::Idle,
        Ok(UploadStatus::Waiting) => UploadView::Waiting,
        Ok(UploadStatus::Sending { sent, total }) => UploadView::Sending { sent, total },
        Ok(UploadStatus::Completed { bytes, http_status }) => {
            UploadView::Completed { bytes, http_status }
        }
        Ok(UploadStatus::Cancelled) => UploadView::Cancelled,
        Ok(UploadStatus::Failed(f)) => UploadView::Failed(failure_view(f)),
        Err(DeviceError::Unsupported(_)) => UploadView::Unsupported,
        Err(_) => UploadView::ReadFailed,
    }
}

/// 現在の OS 向け実装。初期化に失敗したらダミーの成功ではなく `Unsupported` に倒す。
fn implementation() -> Arc<dyn BackgroundUpload> {
    static INSTANCE: OnceLock<Arc<dyn BackgroundUpload>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                match super::apple_background_upload::instance() {
                    Some(d) => d,
                    None => Arc::new(fandhe_device::background_upload::UnsupportedBackgroundUpload),
                }
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_background_upload::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_background_upload::implementation()
            }
            #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
            {
                Arc::new(fandhe_device::background_upload::UnsupportedBackgroundUpload)
            }
        })
        .clone()
}

/// OS 呼び出しを直列に流す専用スレッドへ仕事を渡す（契約表 11）。
fn enqueue(task: Task) {
    static WORKER: OnceLock<Sender<Task>> = OnceLock::new();
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = channel::<Task>();
        std::thread::Builder::new()
            .name("background-upload".to_string())
            .spawn(move || {
                while let Ok(task) = rx.recv() {
                    task();
                }
            })
            .expect("background-upload worker");
        tx
    });
    let _ = tx.send(task);
}

/// 要求 1 件を直列ワーカーで処理し、完了を送る。要求が無ければ `false`。
///
/// 開始の前には `status()` を OS の正として読み、待機中・送信中なら `start` を呼ばない（連打しても重ならない）。
/// 完了は `finish_action` と最新の状態を 1 つの closure で反映する。
fn drive_request(
    state: &BackgroundUploadState,
    sender: &CompletionSender<BackgroundUploadState>,
    imp: Arc<dyn BackgroundUpload>,
    url: &'static str,
    queue: &dyn Fn(Task),
) -> bool {
    let Some(request) = take_request(state) else {
        return false;
    };
    let _ = sender.send(Box::new(mark_in_flight));
    let generation = request.generation;
    let action = request.action;
    let sender = sender.clone();
    queue(Box::new(move || {
        let result = match action {
            Action::Start
                if matches!(
                    imp.status(),
                    Ok(UploadStatus::Waiting | UploadStatus::Sending { .. })
                ) =>
            {
                Ok(())
            }
            Action::Start => imp.start(url).map_err(start_failure),
            Action::Cancel => imp.cancel().map_err(cancel_failure),
        };
        let view = to_view(imp.status());
        let _ = sender.send(Box::new(move |s: &mut BackgroundUploadState| {
            finish_action(s, generation, result);
            apply_snapshot(s, view);
        }));
    }));
    true
}

/// OS の一覧の読み直しを積む。画面を開いた直後か合図が届いているときだけで、積み済みなら畳む。
fn drive_read(
    state: &BackgroundUploadState,
    sender: &CompletionSender<BackgroundUploadState>,
    imp: Arc<dyn BackgroundUpload>,
    gate: &'static ReadGate,
    queue: &dyn Fn(Task),
) {
    if !(needs_read(state) || gate.pending.load(Ordering::SeqCst)) {
        return;
    }
    if gate.queued.swap(true, Ordering::SeqCst) {
        return;
    }
    let sender = sender.clone();
    queue(Box::new(move || {
        // 着手の時点で畳みを解く。読む間に届いた合図は `pending` に残り、次のフレームで読み直す。
        gate.queued.store(false, Ordering::SeqCst);
        gate.pending.store(false, Ordering::SeqCst);
        let view = to_view(imp.status());
        let _ = sender.send(Box::new(move |s: &mut BackgroundUploadState| {
            apply_snapshot(s, view);
        }));
    }));
}

/// effect 1 回分。合図の登録 → 要求の処理 → 読み直し、の順で、周期の予約はしない。
fn drive(
    state: &BackgroundUploadState,
    sender: &CompletionSender<BackgroundUploadState>,
    imp: &Arc<dyn BackgroundUpload>,
    gate: &'static ReadGate,
    queue: &dyn Fn(Task),
) {
    if drive_request(state, sender, imp.clone(), UPLOAD_URL, queue) {
        return;
    }
    drive_read(state, sender, imp.clone(), gate, queue);
}

/// `registry.rs` の `effect` 区画から配線される effect ドライバ。
pub fn background_upload_effect(
    state: &BackgroundUploadState,
    sender: &CompletionSender<BackgroundUploadState>,
) {
    let imp = implementation();
    register_signal(&imp, sender);
    drive(state, sender, &imp, &GATE, &enqueue);
}

/// OS の合図を 1 度だけ登録する。合図は `pending` を立て、初めてのときだけ空の完了を送って UI を起こす
/// （OS コールバックスレッドから egui を起こす経路）。登録できない（非対応）ときは何もしない。
fn register_signal(
    imp: &Arc<dyn BackgroundUpload>,
    sender: &CompletionSender<BackgroundUploadState>,
) {
    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    let wake = sender.clone();
    let _ = imp.set_signal(Arc::new(move || {
        if GATE.mark_pending() {
            let _ = wake.send(Box::new(|_: &mut BackgroundUploadState| {}));
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::background_upload::{cancel, start, Phase};
    use fandhe_device::background_upload::OnSignal;
    use fandhe_device::http::check_url;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    fn sender_with_channel() -> (
        CompletionSender<BackgroundUploadState>,
        std::sync::mpsc::Receiver<crate::app::Completion<BackgroundUploadState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn sync_queue(task: Task) {
        task();
    }

    /// 状態と呼び出し回数を持つモック。
    struct Mock {
        unsupported: bool,
        status: Mutex<Result<UploadStatus, DeviceError>>,
        starts: AtomicUsize,
        reads: AtomicUsize,
    }

    impl Mock {
        fn with(status: Result<UploadStatus, DeviceError>) -> Arc<Self> {
            Arc::new(Self {
                unsupported: false,
                status: Mutex::new(status),
                starts: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
            })
        }
    }

    impl BackgroundUpload for Mock {
        fn start(&self, _url: &str) -> Result<(), DeviceError> {
            if self.unsupported {
                return Err(DeviceError::Unsupported("background upload"));
            }
            self.starts.fetch_add(1, Ordering::SeqCst);
            *self.status.lock().expect("lock") = Ok(UploadStatus::Waiting);
            Ok(())
        }
        fn cancel(&self) -> Result<(), DeviceError> {
            if self.unsupported {
                return Err(DeviceError::Unsupported("background upload"));
            }
            Ok(())
        }
        fn status(&self) -> Result<UploadStatus, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match &*self.status.lock().expect("lock") {
                Ok(s) => Ok(s.clone()),
                Err(DeviceError::Unsupported(n)) => Err(DeviceError::Unsupported(n)),
                Err(_) => Err(DeviceError::Internal("background upload")),
            }
        }
        fn set_signal(&self, _on_signal: OnSignal) -> Result<(), DeviceError> {
            Ok(())
        }
    }

    fn run_request(state: &mut BackgroundUploadState, imp: Arc<dyn BackgroundUpload>) {
        let (sender, rx) = sender_with_channel();
        assert!(drive_request(state, &sender, imp, "u", &sync_queue));
        rx.recv().expect("in flight")(state);
        rx.recv().expect("finish")(state);
    }

    #[test]
    fn sending_upload_is_not_restarted() {
        let mut state = BackgroundUploadState::default();
        start(&mut state);
        let imp = Mock::with(Ok(UploadStatus::Sending {
            sent: 1,
            total: None,
        }));
        run_request(&mut state, imp.clone());
        assert_eq!(imp.starts.load(Ordering::SeqCst), 0);
        assert_eq!(state.error, None);
        assert_eq!(state.phase, Phase::Idle);
    }

    #[test]
    fn start_finishes_with_fresh_view_in_one_step() {
        let mut state = BackgroundUploadState::default();
        start(&mut state);
        let imp = Mock::with(Ok(UploadStatus::Idle));
        run_request(&mut state, imp.clone());
        assert_eq!(imp.starts.load(Ordering::SeqCst), 1);
        // 開始と最新の状態が 1 つの closure で反映される。
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.view, UploadView::Waiting);
    }

    #[test]
    fn unsupported_start_becomes_fixed_error_not_success() {
        let mut state = BackgroundUploadState::default();
        start(&mut state);
        let imp = Arc::new(Mock {
            unsupported: true,
            status: Mutex::new(Err(DeviceError::Unsupported("background upload"))),
            starts: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
        });
        run_request(&mut state, imp);
        assert_eq!(state.error, Some(ActionFailure::Unsupported));
        assert_eq!(state.view, UploadView::Unsupported);
        assert_eq!(state.phase, Phase::Idle);
    }

    #[test]
    fn cancel_success_leaves_no_error() {
        let mut state = BackgroundUploadState::default();
        cancel(&mut state);
        run_request(&mut state, Mock::with(Ok(UploadStatus::Cancelled)));
        assert_eq!(state.error, None);
        assert_eq!(state.view, UploadView::Cancelled);
    }

    #[test]
    fn status_error_is_read_failed_not_idle() {
        assert_eq!(
            to_view(Err(DeviceError::Internal("x"))),
            UploadView::ReadFailed
        );
        assert_eq!(
            to_view(Err(DeviceError::Unsupported("x"))),
            UploadView::Unsupported
        );
    }

    #[test]
    fn device_errors_map_to_fixed_failures() {
        assert_eq!(
            start_failure(DeviceError::Http("x".into())),
            ActionFailure::InvalidDestination
        );
        assert_eq!(
            start_failure(DeviceError::InvalidInput("x")),
            ActionFailure::InvalidDestination
        );
        assert_eq!(
            start_failure(DeviceError::Internal(FILE_ERROR)),
            ActionFailure::FileUnavailable
        );
        assert_eq!(
            start_failure(DeviceError::Internal("other")),
            ActionFailure::StartFailed
        );
        assert_eq!(
            cancel_failure(DeviceError::Internal("x")),
            ActionFailure::CancelFailed
        );
    }

    #[test]
    fn signals_collapse_into_one_read_and_one_wake() {
        static GATE_T: ReadGate = ReadGate {
            pending: AtomicBool::new(false),
            queued: AtomicBool::new(false),
        };
        let wakes = (0..100).filter(|_| GATE_T.mark_pending()).count();
        assert_eq!(wakes, 1);
        let (sender, rx) = sender_with_channel();
        let imp = Mock::with(Ok(UploadStatus::Idle));
        let state = BackgroundUploadState {
            view: UploadView::Idle,
            ..Default::default()
        };
        // 同期キューだと着手まで済むので、積むだけのキューで 100 回呼んでも 1 件になることを見る。
        let held: Mutex<Vec<Task>> = Mutex::new(Vec::new());
        let hold = |t: Task| held.lock().expect("lock").push(t);
        for _ in 0..100 {
            drive_read(&state, &sender, imp.clone(), &GATE_T, &hold);
        }
        assert_eq!(held.lock().expect("lock").len(), 1);
        let task = held.lock().expect("lock").pop().expect("task");
        task();
        assert_eq!(imp.reads.load(Ordering::SeqCst), 1);
        assert!(!GATE_T.pending.load(Ordering::SeqCst));
        let mut state = state;
        rx.recv().expect("snapshot")(&mut state);
        // 合図が無ければ読み直さない（周期の予約をしない）。
        drive_read(&state, &sender, imp.clone(), &GATE_T, &sync_queue);
        assert_eq!(imp.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn first_open_reads_once_even_without_signal() {
        static GATE_T: ReadGate = ReadGate {
            pending: AtomicBool::new(false),
            queued: AtomicBool::new(false),
        };
        let (sender, rx) = sender_with_channel();
        let imp = Mock::with(Ok(UploadStatus::Idle));
        let mut state = BackgroundUploadState::default();
        drive_read(&state, &sender, imp.clone(), &GATE_T, &sync_queue);
        rx.recv().expect("snapshot")(&mut state);
        assert_eq!(state.view, UploadView::Idle);
        drive_read(&state, &sender, imp.clone(), &GATE_T, &sync_queue);
        assert_eq!(imp.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unsupported_first_read_stops_reading() {
        static GATE_T: ReadGate = ReadGate {
            pending: AtomicBool::new(false),
            queued: AtomicBool::new(false),
        };
        let (sender, rx) = sender_with_channel();
        let imp = Mock::with(Err(DeviceError::Unsupported("background upload")));
        let mut state = BackgroundUploadState::default();
        drive_read(&state, &sender, imp.clone(), &GATE_T, &sync_queue);
        rx.recv().expect("snapshot")(&mut state);
        assert_eq!(state.view, UploadView::Unsupported);
        drive_read(&state, &sender, imp.clone(), &GATE_T, &sync_queue);
        assert_eq!(imp.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn drive_without_request_or_need_is_noop() {
        let (sender, rx) = sender_with_channel();
        let state = BackgroundUploadState {
            view: UploadView::Idle,
            ..Default::default()
        };
        assert!(!drive_request(
            &state,
            &sender,
            Mock::with(Ok(UploadStatus::Idle)),
            "u",
            &sync_queue
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn default_url_passes_check_url() {
        assert!(check_url(UPLOAD_URL).is_ok());
    }
}
