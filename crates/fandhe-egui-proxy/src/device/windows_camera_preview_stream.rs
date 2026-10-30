//! Windows 向けカメラのライブ映像実装（Issue #1102）。`device/camera_preview_stream.rs` の
//! `platform_camera_preview`（Windows 経路）から呼ばれ、WinRT `MediaCapture` + `MediaFrameReader`
//! で Bgra8 のフレームを受け、縮小して `deliver_frame` で渡す。契約と守る範囲は
//! `fandhe_device::camera_preview_stream` のモジュール doc、状態の写像（HRESULT・開始 status・
//! `CameraStreamState`）はそこの純関数が SSOT。
//!
//! # 流れ（専用ワーカーの待たない状態機械）
//!
//! 1. `DeviceInformation::FindAllAsyncDeviceClass(VideoCapture)`: 背面（`Panel::Back`）のカメラを
//!    探すだけ。列挙の失敗は開始失敗にせず、OS の既定カメラを使う（事前判定をしない）
//! 2. `InitializeWithSettingsAsync`（Video・`MemoryPreference::Cpu`・`ExclusiveControl`）。失敗は
//!    HRESULT を写す。成功後に `Failed` / `CameraStreamStateChanged` を登録する
//! 3. 色のフレームソース（`VideoPreview` を優先）から `CreateFrameReaderWithSubtypeAsync(Bgra8)`
//! 4. `FrameArrived` を登録して `StartAsync`。**映像中の確定は status が `Success` のとき**
//!
//! 各段の完了は `SetCompleted` から [`Cmd`] としてキューへ積む。ワーカーが待つのは `rx.recv()` だけ
//! （解放時の `StopAsync` を除く）。`FrameArrived` は `AtomicBool` で畳み込み、`FrameReady` を 1 件だけ
//! 積む。フレームは `TryAcquireLatestFrame` で取り、`MediaFrameReference` と `SoftwareBitmap` は
//! 全経路で `Close` する（Dispose しないと供給が止まる。公式）。`unsafe` と `RoInitialize` は使わない。
//!
//! # 並行性・解放（契約表 2・9・11）
//!
//! 開始・停止・ズーム・各段の完了・OS の通知は直列キュー 1 本に集約し、状態はワーカーのローカル
//! 変数にだけ置く。セッションは `id`（ドライバの登録表が発行）で照合し、古い `id` の完了は捨てる。
//! 停止（[`WindowsSession`] の drop）は、登録解除 → `Stop` を積んでハンドラの解除と保留操作の
//! `Cancel` を待つ → 以後コールバックは走らない。`StopAsync` / `Close` はその後ワーカー上で行い、
//! UI は待たない。解放呼び出しの失敗は握りつぶさず、未解放として [`Worker::leftover`] に残し、
//! 次の開始の前に再試行する（それも失敗なら新しい開始を `StartFailed(Other)` にする）。
//!
//! 画素・HRESULT・デバイス Id はログにもエラー文言にも出さない（security.md A09）。
//!
//! # 既知の天井
//!
//! `InitializeAsync` は公式には STA 指定で、本実装は mic-record と同じく MTA のワーカーで呼ぶ（実機で
//! 失敗したら UI スレッドでの初期化へ寄せる。#808）。初期化中の `Close` の挙動、高解像度での縮小の
//! 負荷は実機未確認。型検査は `x86_64-pc-windows-gnu` のみ。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};

use windows::core::{IInspectable, Interface, Ref, HRESULT, HSTRING};
use windows::Devices::Enumeration::{
    DeviceClass, DeviceInformation, DeviceInformationCollection, Panel,
};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Capture::Frames::{
    MediaFrameReader, MediaFrameReaderAcquisitionMode, MediaFrameReaderStartStatus,
    MediaFrameSource, MediaFrameSourceKind,
};
use windows::Media::Capture::MediaStreamType;
use windows::Media::Capture::{
    MediaCapture, MediaCaptureFailedEventHandler, MediaCaptureInitializationSettings,
    MediaCaptureMemoryPreference, MediaCaptureSharingMode, StreamingCaptureMode,
};
use windows::Media::Devices::{ZoomControl, ZoomSettings, ZoomTransitionMode};
use windows::Media::MediaProperties::MediaEncodingSubtypes;
use windows::Storage::Streams::{Buffer, DataReader};
use windows_future::{
    AsyncActionCompletedHandler, AsyncOperationCompletedHandler, AsyncStatus, IAsyncAction,
    IAsyncInfo, IAsyncOperation,
};

use fandhe_device::camera_preview_stream::{
    checked_zoom, downscale_packed, event_from_windows_stream_state,
    start_failure_from_windows_hresult, start_failure_from_windows_reader_status, windows_bgra_len,
    zoom_target, CameraPreview, PackedLayout, PreviewEvent, PreviewFrame, PreviewSession,
    PreviewSink, PreviewStartError, StartFailure, ZoomFailure, ZoomStep,
};

use super::camera_preview_stream::{deliver_event, deliver_frame, register, unregister};

/// `E_FAIL`（完了が成功以外で、OS が理由を返さないときの代用）。
const E_FAIL: i32 = 0x8000_4005_u32 as i32;

/// ワーカーへの命令。開始・停止・ズーム・各段の完了・OS の通知はすべてここを順に通る。
enum Cmd {
    Start(u64),
    /// ハンドラの解除と保留操作の取り消しが済んだら `ack` を返す。
    Stop(u64, Sender<()>),
    Zoom(u64, ZoomStep),
    Devices(u64, windows::core::Result<DeviceInformationCollection>),
    Initialized(u64, windows::core::Result<()>),
    Reader(u64, windows::core::Result<MediaFrameReader>),
    ReaderStarted(u64, windows::core::Result<MediaFrameReaderStartStatus>),
    FrameReady(u64),
    StreamState(u64),
    CaptureFailed(u64),
}

/// 1 回の開始から解放までの WinRT 資源。解放済みの項目は `None` / `true` にして二重に外さない。
struct Session {
    id: u64,
    capture: MediaCapture,
    reader: Option<MediaFrameReader>,
    /// 保留中の非同期操作（停止・失敗で `Cancel` する先）。
    op: Option<IAsyncInfo>,
    failed_token: Option<i64>,
    state_token: Option<i64>,
    frame_token: Option<i64>,
    /// `FrameArrived` の畳み込み用。ワーカーが下ろしてからフレームを取る。
    frame_flag: Arc<AtomicBool>,
    zoom: Option<ZoomControl>,
    /// `StartAsync` が `Success` で確定したか。
    started: bool,
    /// 中断中か（`Streaming` への復帰を `Running` として通すため）。
    interrupted: bool,
    reader_stopped: bool,
    reader_closed: bool,
    capture_closed: bool,
}

impl Session {
    /// ハンドラの解除と保留操作の取り消し。以後コールバックは `Cmd` を積むだけで無害になる。
    fn detach(&mut self) -> bool {
        let mut ok = true;
        if let Some(op) = self.op.take() {
            let _ = op.Cancel();
        }
        if let Some(t) = self.failed_token {
            if self.capture.RemoveFailed(t).is_ok() {
                self.failed_token = None;
            } else {
                ok = false;
            }
        }
        if let Some(t) = self.state_token {
            if self.capture.RemoveCameraStreamStateChanged(t).is_ok() {
                self.state_token = None;
            } else {
                ok = false;
            }
        }
        if let (Some(t), Some(r)) = (self.frame_token, &self.reader) {
            if r.RemoveFrameArrived(t).is_ok() {
                self.frame_token = None;
            } else {
                ok = false;
            }
        }
        ok
    }

    /// 全項目を解放する。1 件でも失敗したら `false` で、残りは次回に再試行する。
    fn release(&mut self) -> bool {
        let mut ok = self.detach();
        if let Some(r) = &self.reader {
            if !ok {
                return false;
            }
            if !self.reader_stopped {
                // 解放の順序のため、ワーカー上で完了を待つ（UI は待たない）。
                match r.StopAsync().and_then(|op| op.get()) {
                    Ok(()) => self.reader_stopped = true,
                    Err(_) => ok = false,
                }
            }
            if self.reader_stopped && !self.reader_closed {
                if r.Close().is_ok() {
                    self.reader_closed = true;
                } else {
                    ok = false;
                }
            }
        }
        // 子（ハンドラ・reader）の解除・停止・Close が全部済んでから親を閉じる。
        // 先に閉じると、未解除の項目が閉鎖済みエラーになり再試行が成立しない。
        if ok && !self.capture_closed {
            if self.capture.Close().is_ok() {
                self.capture_closed = true;
            } else {
                ok = false;
            }
        }
        ok
    }
}

/// `IAsyncOperation` の完了を `wrap` で [`Cmd`] にしてキューへ積む。待たない。
fn spawn<T: windows::core::RuntimeType + 'static>(
    op: windows::core::Result<IAsyncOperation<T>>,
    tx: &Sender<Cmd>,
    wrap: impl Fn(windows::core::Result<T>) -> Cmd + Send + 'static,
) -> windows::core::Result<IAsyncInfo> {
    let op = op?;
    let tx = tx.clone();
    op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
        let result = match (op.ok(), status) {
            (Ok(op), AsyncStatus::Completed) => op.GetResults(),
            // 失敗・取り消しは成功にしない（理由が取れなければ E_FAIL）。
            (Ok(op), _) => Err(op
                .GetResults()
                .err()
                .unwrap_or_else(|| HRESULT(E_FAIL).into())),
            _ => Err(HRESULT(E_FAIL).into()),
        };
        let _ = tx.send(wrap(result));
        Ok(())
    }))?;
    op.cast()
}

/// `IAsyncAction` 版の [`spawn`]。
fn spawn_action(
    op: windows::core::Result<IAsyncAction>,
    tx: &Sender<Cmd>,
    wrap: impl Fn(windows::core::Result<()>) -> Cmd + Send + 'static,
) -> windows::core::Result<IAsyncInfo> {
    let op = op?;
    let tx = tx.clone();
    op.SetCompleted(&AsyncActionCompletedHandler::new(move |op, status| {
        let result = match (op.ok(), status) {
            (Ok(op), AsyncStatus::Completed) => op.GetResults(),
            (Ok(op), _) => Err(op
                .GetResults()
                .err()
                .unwrap_or_else(|| HRESULT(E_FAIL).into())),
            _ => Err(HRESULT(E_FAIL).into()),
        };
        let _ = tx.send(wrap(result));
        Ok(())
    }))?;
    op.cast()
}

struct WindowsCameraPreview;

static WORKER: OnceLock<Option<Sender<Cmd>>> = OnceLock::new();

fn worker() -> Option<&'static Sender<Cmd>> {
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            let own = tx.clone();
            std::thread::Builder::new()
                .name("camera-preview".into())
                .spawn(move || run_worker(rx, own))
                .ok()
                .map(|_| tx)
        })
        .as_ref()
}

impl CameraPreview for WindowsCameraPreview {
    fn start(
        &self,
        sink: Arc<dyn PreviewSink>,
    ) -> Result<Box<dyn PreviewSession>, PreviewStartError> {
        let id = register(sink);
        if worker().is_some_and(|tx| tx.send(Cmd::Start(id)).is_ok()) {
            Ok(Box::new(WindowsSession { id }))
        } else {
            unregister(id);
            Err(PreviewStartError::Failed(StartFailure::Other))
        }
    }
}

/// 開始した映像 1 件。drop で停止し、戻った後はコールバックが走らない。
struct WindowsSession {
    id: u64,
}

impl PreviewSession for WindowsSession {
    fn set_zoom(&self, step: ZoomStep) -> Result<(), ZoomFailure> {
        match worker() {
            Some(tx) if tx.send(Cmd::Zoom(self.id, step)).is_ok() => Ok(()),
            _ => Err(ZoomFailure::Failed),
        }
    }
}

impl Drop for WindowsSession {
    fn drop(&mut self) {
        unregister(self.id);
        // ワーカー未起動なら動いていない。解除と取り消しが済むまで待つ（解放の本体は待たない）。
        let Some(Some(tx)) = WORKER.get() else {
            return;
        };
        let (ack_tx, ack_rx) = mpsc::channel();
        if tx.send(Cmd::Stop(self.id, ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }
}

/// `device/camera_preview_stream.rs` から呼ばれる、プロセスに 1 つの実装。
pub(crate) fn implementation() -> Arc<dyn CameraPreview> {
    Arc::new(WindowsCameraPreview)
}

#[derive(Default)]
struct Worker {
    current: Option<Session>,
    /// 解放に失敗して残った分。要求より長く生き、次の開始の前に再試行する（契約表 9）。
    leftover: Option<Session>,
    /// 停止後に届いた reader で、Close に失敗した分。次の開始の前に再試行する（契約表 9）。
    stray: Vec<MediaFrameReader>,
}

fn run_worker(rx: mpsc::Receiver<Cmd>, tx: Sender<Cmd>) {
    let mut w = Worker::default();
    while let Ok(cmd) = rx.recv() {
        w.step(cmd, &tx);
    }
}

impl Worker {
    fn cur(&mut self, id: u64) -> Option<&mut Session> {
        self.current.as_mut().filter(|s| s.id == id)
    }

    /// 現在のセッションを解放する。失敗は `leftover` に残す。
    fn dispose(&mut self) {
        if let Some(mut s) = self.current.take() {
            if !s.release() {
                self.leftover = Some(s);
            }
        }
    }

    /// 終端イベントを届けて解放する（開始失敗・実行中の失敗）。
    fn end(&mut self, id: u64, event: PreviewEvent) {
        deliver_event(id, event);
        self.dispose();
    }

    fn step(&mut self, cmd: Cmd, tx: &Sender<Cmd>) {
        match cmd {
            Cmd::Start(id) => self.begin(id, tx),
            Cmd::Stop(id, ack) => {
                if let Some(s) = self.cur(id) {
                    s.detach();
                }
                let _ = ack.send(());
                if self.cur(id).is_some() {
                    self.dispose();
                }
            }
            Cmd::Zoom(id, step) => {
                if let Some(s) = self.cur(id) {
                    // 開始確定前の要求は黙って捨てず、失敗として通知する。
                    let r = if s.started {
                        apply_zoom(s, step)
                    } else {
                        Err(ZoomFailure::Failed)
                    };
                    if let Err(f) = r {
                        deliver_event(id, PreviewEvent::ZoomFailed(f));
                    }
                }
            }
            Cmd::Devices(id, found) => self.initialize(id, found, tx),
            Cmd::Initialized(id, result) => self.after_init(id, result, tx),
            Cmd::Reader(id, result) => self.after_reader(id, result, tx),
            Cmd::ReaderStarted(id, result) => self.after_started(id, result),
            Cmd::FrameReady(id) => self.frame(id),
            Cmd::StreamState(id) => self.stream_state(id),
            Cmd::CaptureFailed(id) => {
                if self.cur(id).is_some() {
                    self.end(id, PreviewEvent::RuntimeFailed);
                }
            }
        }
    }

    fn begin(&mut self, id: u64, tx: &Sender<Cmd>) {
        // 前の分を片付けてから開始する。解放できないまま開始しない（契約表 9）。
        self.dispose();
        if let Some(mut old) = self.leftover.take() {
            if !old.release() {
                self.leftover = Some(old);
                deliver_event(id, PreviewEvent::StartFailed(StartFailure::Other));
                return;
            }
        }
        self.stray.retain(|r| r.Close().is_err());
        if !self.stray.is_empty() {
            deliver_event(id, PreviewEvent::StartFailed(StartFailure::Other));
            return;
        }
        let Ok(capture) = MediaCapture::new() else {
            deliver_event(id, PreviewEvent::StartFailed(StartFailure::Other));
            return;
        };
        let op = spawn(
            DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::VideoCapture),
            tx,
            move |r| Cmd::Devices(id, r),
        );
        let Ok(op) = op else {
            // 起動できなかった MediaCapture は閉じて確かめる。
            let mut s = new_session(id, capture, None);
            if !s.release() {
                self.leftover = Some(s);
            }
            deliver_event(id, PreviewEvent::StartFailed(StartFailure::Other));
            return;
        };
        self.current = Some(new_session(id, capture, Some(op)));
    }

    fn initialize(
        &mut self,
        id: u64,
        found: windows::core::Result<DeviceInformationCollection>,
        tx: &Sender<Cmd>,
    ) {
        let Some(s) = self.cur(id) else { return };
        // 列挙の失敗は優先順位が決まらないだけで、OS の既定カメラで続ける。
        let back = found.ok().and_then(|c| back_camera_id(&c));
        let started = (|| {
            let settings = MediaCaptureInitializationSettings::new()?;
            settings.SetStreamingCaptureMode(StreamingCaptureMode::Video)?;
            settings.SetMemoryPreference(MediaCaptureMemoryPreference::Cpu)?;
            settings.SetSharingMode(MediaCaptureSharingMode::ExclusiveControl)?;
            if let Some(dev) = back {
                settings.SetVideoDeviceId(&dev)?;
            }
            spawn_action(
                s.capture.InitializeWithSettingsAsync(&settings),
                tx,
                move |r| Cmd::Initialized(id, r),
            )
        })();
        match started {
            Ok(op) => s.op = Some(op),
            Err(e) => {
                let f = start_failure_from_windows_hresult(e.code().0);
                self.end(id, PreviewEvent::StartFailed(f));
            }
        }
    }

    fn after_init(&mut self, id: u64, result: windows::core::Result<()>, tx: &Sender<Cmd>) {
        let Some(s) = self.cur(id) else { return };
        s.op = None;
        if let Err(e) = result {
            let f = start_failure_from_windows_hresult(e.code().0);
            self.end(id, PreviewEvent::StartFailed(f));
            return;
        }
        let next = (|| {
            let ftx = tx.clone();
            s.failed_token = Some(s.capture.Failed(&MediaCaptureFailedEventHandler::new(
                move |_, _| {
                    let _ = ftx.send(Cmd::CaptureFailed(id));
                    Ok(())
                },
            ))?);
            let stx = tx.clone();
            s.state_token = Some(s.capture.CameraStreamStateChanged(&TypedEventHandler::<
                MediaCapture,
                IInspectable,
            >::new(
                move |_, _| {
                    let _ = stx.send(Cmd::StreamState(id));
                    Ok(())
                },
            ))?);
            let source = color_source(&s.capture)?;
            let subtype = MediaEncodingSubtypes::Bgra8()?;
            spawn(
                s.capture
                    .CreateFrameReaderWithSubtypeAsync(&source, &subtype),
                tx,
                move |r| Cmd::Reader(id, r),
            )
        })();
        match next {
            Ok(op) => s.op = Some(op),
            // ソースが無い（空エラー code 0 を含む）はカメラなし。
            Err(e) if e.code().0 == 0 => {
                self.end(id, PreviewEvent::StartFailed(StartFailure::NoCamera))
            }
            Err(e) => {
                let f = start_failure_from_windows_hresult(e.code().0);
                self.end(id, PreviewEvent::StartFailed(f));
            }
        }
    }

    fn after_reader(
        &mut self,
        id: u64,
        result: windows::core::Result<MediaFrameReader>,
        tx: &Sender<Cmd>,
    ) {
        if self.cur(id).is_none() {
            // 停止・差し替え後に届いた reader。drop では Close にならないので明示的に閉じる。
            if let Ok(r) = result {
                if r.Close().is_err() {
                    self.stray.push(r);
                }
            }
            return;
        }
        let Some(s) = self.cur(id) else { return };
        s.op = None;
        let reader = match result {
            Ok(r) => r,
            Err(e) => {
                let f = start_failure_from_windows_hresult(e.code().0);
                self.end(id, PreviewEvent::StartFailed(f));
                return;
            }
        };
        s.reader = Some(reader.clone());
        let next = (|| {
            reader.SetAcquisitionMode(MediaFrameReaderAcquisitionMode::Realtime)?;
            let (flag, ftx) = (s.frame_flag.clone(), tx.clone());
            s.frame_token = Some(reader.FrameArrived(&TypedEventHandler::new(
                move |_: Ref<'_, MediaFrameReader>, _| {
                    if !flag.swap(true, Ordering::SeqCst) {
                        let _ = ftx.send(Cmd::FrameReady(id));
                    }
                    Ok(())
                },
            ))?);
            spawn(reader.StartAsync(), tx, move |r| Cmd::ReaderStarted(id, r))
        })();
        match next {
            Ok(op) => s.op = Some(op),
            Err(e) => {
                let f = start_failure_from_windows_hresult(e.code().0);
                self.end(id, PreviewEvent::StartFailed(f));
            }
        }
    }

    fn after_started(
        &mut self,
        id: u64,
        result: windows::core::Result<MediaFrameReaderStartStatus>,
    ) {
        let Some(s) = self.cur(id) else { return };
        s.op = None;
        let failure = match result {
            Ok(status) => start_failure_from_windows_reader_status(status.0),
            Err(e) => Some(start_failure_from_windows_hresult(e.code().0)),
        };
        if let Some(f) = failure {
            self.end(id, PreviewEvent::StartFailed(f));
            return;
        }
        // 開始の確定。ズームは対応するカメラだけ保持する。
        s.zoom = s
            .capture
            .VideoDeviceController()
            .and_then(|c| c.ZoomControl())
            .ok()
            .filter(|z| z.Supported().unwrap_or(false));
        s.started = true;
        deliver_event(id, PreviewEvent::Running);
        // 開始確定前に届いた状態通知は無視されているため、現在の状態で一度評価し直す。
        self.stream_state(id);
    }

    fn stream_state(&mut self, id: u64) {
        let Some(s) = self.cur(id) else { return };
        let Ok(state) = s.capture.CameraStreamState() else {
            return;
        };
        match event_from_windows_stream_state(state.0) {
            Some(PreviewEvent::Running) if s.started && s.interrupted => {
                s.interrupted = false;
                deliver_event(id, PreviewEvent::Running);
            }
            Some(PreviewEvent::Interrupted) if s.started && !s.interrupted => {
                s.interrupted = true;
                deliver_event(id, PreviewEvent::Interrupted);
            }
            Some(PreviewEvent::RuntimeFailed) => self.end(id, PreviewEvent::RuntimeFailed),
            _ => {}
        }
    }

    fn frame(&mut self, id: u64) {
        let Some(s) = self.cur(id) else { return };
        // FrameArrived は開始確定より先に届きうる。started 判定より前に畳み込み旗を戻し、
        // 以後の FrameReady が積まれなくなる停止を防ぐ。
        s.frame_flag.store(false, Ordering::SeqCst);
        if !s.started {
            return;
        }
        let Some(reader) = &s.reader else { return };
        // null（空エラー code 0）は「新しいフレームなし」。それ以外のエラーは実際の失敗。
        let fr = match reader.TryAcquireLatestFrame() {
            Ok(f) => f,
            Err(e) if e.code().0 == 0 => return,
            Err(_) => {
                self.end(id, PreviewEvent::RuntimeFailed);
                return;
            }
        };
        let bmp = fr.VideoMediaFrame().and_then(|v| v.SoftwareBitmap());
        let (frame, mut ok) = match &bmp {
            Ok(b) => {
                let (f, closed) = convert(b, s.zoom.as_ref());
                {
                    let ok = f.is_some() && closed;
                    (f, ok)
                }
            }
            // ビットマップが無いフレーム（null）は届けるものがないだけ。
            Err(e) if e.code().0 == 0 => (None, true),
            Err(_) => (None, false),
        };
        if let Ok(b) = &bmp {
            ok &= b.Close().is_ok();
        }
        ok &= fr.Close().is_ok();
        if !ok {
            // 取得・変換・コピーの失敗、または閉じられず供給が止まる場合は失敗として確定する。
            self.end(id, PreviewEvent::RuntimeFailed);
            return;
        }
        if let Some(f) = frame {
            deliver_frame(id, f);
        }
    }
}

fn new_session(id: u64, capture: MediaCapture, op: Option<IAsyncInfo>) -> Session {
    Session {
        id,
        capture,
        reader: None,
        op,
        failed_token: None,
        state_token: None,
        frame_token: None,
        frame_flag: Arc::new(AtomicBool::new(false)),
        zoom: None,
        started: false,
        interrupted: false,
        reader_stopped: false,
        reader_closed: false,
        capture_closed: false,
    }
}

/// 背面のカメラの Id。無ければ `None`（OS の既定カメラを使う）。
fn back_camera_id(devices: &DeviceInformationCollection) -> Option<HSTRING> {
    devices.into_iter().find_map(|d| {
        let panel = d.EnclosureLocation().and_then(|l| l.Panel()).ok()?;
        (panel == Panel::Back).then(|| d.Id().ok()).flatten()
    })
}

/// 色のフレームソース。`VideoPreview` を優先し、無ければ `VideoRecord`。無ければ空エラー（code 0）。
fn color_source(capture: &MediaCapture) -> windows::core::Result<MediaFrameSource> {
    let mut record = None;
    for kv in capture.FrameSources()? {
        let source = kv.Value()?;
        let info = source.Info()?;
        if info.SourceKind()? != MediaFrameSourceKind::Color {
            continue;
        }
        match info.MediaStreamType()? {
            MediaStreamType::VideoPreview => return Ok(source),
            MediaStreamType::VideoRecord if record.is_none() => record = Some(source),
            _ => {}
        }
    }
    record.ok_or_else(|| HRESULT(0).into())
}

fn apply_zoom(s: &Session, step: ZoomStep) -> Result<(), ZoomFailure> {
    let z = s.zoom.as_ref().ok_or(ZoomFailure::Unsupported)?;
    let range = z.Min().ok().zip(z.Max().ok());
    let target = zoom_target(step, range)?;
    let settings = ZoomSettings::new().map_err(|_| ZoomFailure::Failed)?;
    settings
        .SetMode(ZoomTransitionMode::Auto)
        .and_then(|_| settings.SetValue(target))
        .and_then(|_| z.Configure(&settings))
        .map_err(|_| ZoomFailure::Failed)
}

/// `SoftwareBitmap` を縮小済みの `PreviewFrame` にする。`None` は変換・読み出しの失敗。2 つ目は、変換で作った分を含め閉じられたか。
fn convert(bmp: &SoftwareBitmap, zoom: Option<&ZoomControl>) -> (Option<PreviewFrame>, bool) {
    let (src, converted) = if bmp.BitmapPixelFormat().ok() == Some(BitmapPixelFormat::Bgra8) {
        (None, None)
    } else {
        match SoftwareBitmap::Convert(bmp, BitmapPixelFormat::Bgra8) {
            Ok(c) => (Some(c.clone()), Some(c)),
            Err(_) => return (None, true),
        }
    };
    let frame = read_bgra(src.as_ref().unwrap_or(bmp)).and_then(|(w, h, data)| {
        let mut scaled = downscale_packed(w, h, w as usize * 4, PackedLayout::Bgra8, &data)?;
        // 撮像のアルファは意味を持たない（Ignore だと 0 のことがある）ので不透明にそろえる。
        scaled
            .rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|p| p[3] = 255);
        Some(PreviewFrame {
            source_width: w,
            source_height: h,
            frame: scaled,
            zoom: zoom.and_then(|z| z.Value().ok()).and_then(checked_zoom),
        })
    });
    let closed = converted.is_none_or(|c| c.Close().is_ok());
    (frame, closed)
}

/// Bgra8 の `SoftwareBitmap` を `(幅, 高さ, 画素)` で読む。寸法は確保の前に検証する（契約表 5）。
fn read_bgra(bmp: &SoftwareBitmap) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (bmp.PixelWidth().ok()?, bmp.PixelHeight().ok()?);
    let len = windows_bgra_len(w, h)?;
    let buf = Buffer::Create(len).ok()?;
    bmp.CopyToBuffer(&buf).ok()?;
    buf.SetLength(len).ok()?;
    let mut data = vec![0u8; len as usize];
    DataReader::FromBuffer(&buf)
        .ok()?
        .ReadBytes(&mut data)
        .ok()?;
    Some((w as u32, h as u32, data))
}
