//! Windows 向けカメラのライブ映像実装（Issue #1102）。`device/camera_preview_stream.rs` の
//! `platform_camera_preview`（Windows 経路）から呼ばれ、WinRT `MediaCapture` + `MediaFrameReader`
//! で Bgra8 のフレームを受け、縮小して `deliver_frame` で渡す。契約と守る範囲は
//! `fandhe_device::camera_preview_stream` のモジュール doc（「Windows の違い」）、状態の写像
//! （HRESULT・開始 status・`CameraStreamState`）はそこの純関数が SSOT。
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
//! 各段の完了は `SetCompleted` から [`Cmd`] としてキューへ積む（`windows_ble_peripheral.rs` と同じ形）。
//! ワーカーが待つのは `rx.recv()` だけで、ワーカー上で `.get()` は使わない。`FrameArrived` は
//! `AtomicBool` で畳み込み、`FrameReady` を 1 件だけ積む。
//!
//! # 並行性（契約表 2・11）
//!
//! 開始・停止・ズーム・各段の完了・OS の通知は直列キュー（[`Cmd`]）1 本に集約し、状態
//! （[`Worker`]）はワーカーのローカル変数にだけ置く。セッションは `id`（ドライバの登録表が発行）
//! で照合する。停止（[`WindowsSession`] の drop）は、登録解除 → `Stop` を積んで ack を待つ。
//! ワーカーは OS を待たないので ack はすぐ返る。
//!
//! # 解放（契約表 9）
//!
//! 停止・終端した session は `closing` に移り、1 段ずつ解放する（[`Worker::advance`]）。
//! `Remove*` → `StopAsync`（完了は [`Cmd::StopDone`]）→ reader の `Close` → `MediaCapture` の
//! `Close`（子が全部済んでから親）。保留中の操作は `Cancel` するが、戻り値は見ず、完了（取り消しを
//! 含む）が届くまで `closing` から消さない。取り消しと競合して届いた reader もここで閉じる。
//! どの段の失敗も `closing` に残し、次の開始の前に再試行する。前の解放が OS の完了待ちの間に来た
//! 開始は完了まで待たせ、再試行しても済まなければ `StartFailed(Other)` にする。
//!
//! 画素・HRESULT・デバイス Id はログにもエラー文言にも出さない（security.md A09）。
//! `unsafe` と `RoInitialize` は使わない。
//!
//! # 既知の天井
//!
//! `InitializeAsync` は公式には STA 指定で、本実装は MTA のワーカーで呼ぶ（実機で失敗したら UI
//! スレッドでの初期化へ寄せる。#808）。初期化中の `Cancel` の挙動、高解像度での縮小の負荷、
//! ワーカーのホストテスト（WinRT に依存する）は無い。型検査は `x86_64-pc-windows-gnu` のみ。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};

use windows::core::{IInspectable, Interface, Ref, RuntimeType, HRESULT, HSTRING};
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
    /// 受付（保留中の開始の取り下げ・現在の session の解放開始）が済んだら `ack` を返す。
    Stop(u64, Sender<()>),
    Zoom(u64, ZoomStep),
    Devices(u64, windows::core::Result<DeviceInformationCollection>),
    Initialized(u64, windows::core::Result<()>),
    Reader(u64, windows::core::Result<MediaFrameReader>),
    ReaderStarted(u64, windows::core::Result<MediaFrameReaderStartStatus>),
    /// 解放中の `StopAsync` の完了。
    StopDone(u64, windows::core::Result<()>),
    FrameReady(u64),
    StreamState(u64),
    CaptureFailed(u64),
}

/// 非同期操作の完了を運ぶ命令の `id`。これが `closing` の session と一致したら解放の続きとして扱う。
fn completion_id(cmd: &Cmd) -> Option<u64> {
    match cmd {
        Cmd::Devices(id, _)
        | Cmd::Initialized(id, _)
        | Cmd::Reader(id, _)
        | Cmd::ReaderStarted(id, _)
        | Cmd::StopDone(id, _) => Some(*id),
        _ => None,
    }
}

/// reader の解放の段。`NeedsStop` は `StartAsync` を出した後（`StopAsync` が要る）、`NeedsClose` は
/// それ以前・停止済み（`Close` だけ）。始まっていない reader に `StopAsync` を出さないための区別。
enum ReaderSlot {
    NeedsStop(MediaFrameReader),
    NeedsClose(MediaFrameReader),
}

/// 1 回の開始から解放までの WinRT 資源。解放済みの項目は `None` にして二重に外さない。
struct Session {
    id: u64,
    capture: MediaCapture,
    reader: Option<ReaderSlot>,
    /// 保留中の非同期操作。完了の [`Cmd`] が届くまで `None` にしない。
    op: Option<IAsyncInfo>,
    failed_token: Option<i64>,
    state_token: Option<i64>,
    frame_token: Option<i64>,
    /// `FrameArrived` の畳み込み用。ワーカーが下ろしてからフレームを取る。
    frame_flag: Arc<AtomicBool>,
    /// `StartAsync` が `Success` で確定したか。
    started: bool,
    /// 中断中か（`Streaming` への復帰を `Running` として通すため）。
    interrupted: bool,
    /// ズーム制御。`Err` は取得の失敗（`Ok(None)` の非対応とは別。契約表 6）。
    zoom: Result<Option<ZoomControl>, ()>,
}

impl Session {
    fn reader_ref(&self) -> Option<&MediaFrameReader> {
        match &self.reader {
            Some(ReaderSlot::NeedsStop(r) | ReaderSlot::NeedsClose(r)) => Some(r),
            None => None,
        }
    }

    /// `Remove*` を 1 件ずつ実行し、成功したものだけ解放済みにする。全部済んだら `true`。
    fn remove_handlers(&mut self) -> bool {
        let mut ok = true;
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
        if let Some(t) = self.frame_token {
            // token は reader を保持した後にしか立たない。無ければ外す先が無い。
            let removed = self
                .reader_ref()
                .is_none_or(|r| r.RemoveFrameArrived(t).is_ok());
            if removed {
                self.frame_token = None;
            } else {
                ok = false;
            }
        }
        ok
    }
}

/// `IAsyncOperation` の完了を `wrap` で [`Cmd`] にしてキューへ積む。待たない。
fn spawn<T: RuntimeType + 'static>(
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
        // 受信側（ワーカー）が終わっていても無視してよい。
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
        // ワーカー未起動なら動いていない。ワーカーは OS を待たないので、ack はすぐ返る。
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

struct Worker {
    tx: Sender<Cmd>,
    /// 動いている session（開始の途中を含む）。
    current: Option<Session>,
    /// 解放中の session。完了待ち・失敗の持ち越しもここに残る。`Some` の間は `current` は `None`。
    closing: Option<Session>,
    /// `closing` の解放待ちで保留している開始の `id`。
    deferred: Option<u64>,
}

fn run_worker(rx: mpsc::Receiver<Cmd>, tx: Sender<Cmd>) {
    let mut w = Worker {
        tx,
        current: None,
        closing: None,
        deferred: None,
    };
    while let Ok(cmd) = rx.recv() {
        w.step(cmd);
    }
}

impl Worker {
    fn cur(&mut self, id: u64) -> Option<&mut Session> {
        self.current.as_mut().filter(|s| s.id == id)
    }

    /// 解放を始める。ハンドラを外し、保留操作は `Cancel`（戻り値は見ない）して完了を待つ。
    fn close(&mut self, mut s: Session) {
        s.remove_handlers();
        if let Some(op) = &s.op {
            let _ = op.Cancel();
        }
        let idle = s.op.is_none();
        self.closing = Some(s);
        if idle {
            self.advance();
        }
    }

    /// `closing` の解放を 1 段進める。OS の完了は待たない（`StopAsync` は完了を `StopDone` で受ける）。
    fn advance(&mut self) {
        let Some(s) = self.closing.as_mut() else {
            return;
        };
        if s.op.is_some() || !s.remove_handlers() {
            return;
        }
        match s.reader.take() {
            Some(ReaderSlot::NeedsStop(r)) => {
                let id = s.id;
                if let Ok(op) =
                    spawn_action(r.StopAsync(), &self.tx, move |res| Cmd::StopDone(id, res))
                {
                    s.op = Some(op);
                }
                s.reader = Some(ReaderSlot::NeedsStop(r));
                return;
            }
            // Close の失敗は段を変えずに持ち越す。成功した reader はここで手放す。
            Some(ReaderSlot::NeedsClose(r)) if r.Close().is_err() => {
                s.reader = Some(ReaderSlot::NeedsClose(r));
                return;
            }
            Some(ReaderSlot::NeedsClose(_)) | None => {}
        }
        // 子が全部済んでから親を閉じる。
        if s.capture.Close().is_ok() {
            self.closing = None;
        }
    }

    /// `closing` に一致した完了。結果は捨てるが、届いた reader は閉じる段へ回し、`StopDone` の
    /// 成功だけ段を進める（失敗は段を変えず、次の再試行に持ち越す）。
    fn closing_done(&mut self, cmd: Cmd) {
        let Some(s) = self.closing.as_mut() else {
            return;
        };
        s.op = None;
        match cmd {
            Cmd::Reader(_, Ok(r)) => s.reader = Some(ReaderSlot::NeedsClose(r)),
            Cmd::StopDone(_, Ok(())) => {
                if let Some(ReaderSlot::NeedsStop(r)) = s.reader.take() {
                    s.reader = Some(ReaderSlot::NeedsClose(r));
                }
            }
            _ => {}
        }
        self.advance();
        self.resume_deferred();
    }

    /// 解放が済んだら保留中の開始を始める。解放が失敗で止まったなら、その開始を失敗にする。
    fn resume_deferred(&mut self) {
        match &self.closing {
            None => {
                if let Some(id) = self.deferred.take() {
                    self.begin(id);
                }
            }
            Some(s) if s.op.is_none() => {
                if let Some(id) = self.deferred.take() {
                    deliver_event(id, PreviewEvent::StartFailed(StartFailure::Other));
                }
            }
            Some(_) => {}
        }
    }

    fn defer(&mut self, id: u64) {
        if let Some(prev) = self.deferred.replace(id) {
            deliver_event(prev, PreviewEvent::StartFailed(StartFailure::Other));
        }
    }

    /// 終端イベントを届けて解放を始める（開始失敗・実行中の失敗）。
    fn end(&mut self, id: u64, event: PreviewEvent) {
        deliver_event(id, event);
        if let Some(s) = self.current.take_if(|s| s.id == id) {
            self.close(s);
        }
    }

    fn step(&mut self, cmd: Cmd) {
        if let Some(id) = completion_id(&cmd) {
            if self.closing.as_ref().is_some_and(|s| s.id == id) {
                self.closing_done(cmd);
                return;
            }
        }
        match cmd {
            Cmd::Start(id) => self.start(id),
            Cmd::Stop(id, ack) => {
                if self.deferred == Some(id) {
                    self.deferred = None;
                }
                if let Some(s) = self.current.take_if(|s| s.id == id) {
                    self.close(s);
                }
                let _ = ack.send(());
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
            Cmd::Devices(id, found) => self.initialize(id, found),
            Cmd::Initialized(id, result) => self.after_init(id, result),
            Cmd::Reader(id, result) => self.after_reader(id, result),
            Cmd::ReaderStarted(id, result) => self.after_started(id, result),
            // 現在の session の StopDone は起きない（StopAsync は closing でしか出さない）。
            Cmd::StopDone(..) => {}
            Cmd::FrameReady(id) => self.frame(id),
            Cmd::StreamState(id) => self.stream_state(id),
            Cmd::CaptureFailed(id) => {
                if self.cur(id).is_some() {
                    self.end(id, PreviewEvent::RuntimeFailed);
                }
            }
        }
    }

    fn start(&mut self, id: u64) {
        // 順序上は起きないが、上書きによる資源の漏れを防ぐ。
        if let Some(s) = self.current.take() {
            self.close(s);
        }
        if self.closing.is_some() {
            if self.closing.as_ref().is_some_and(|s| s.op.is_some()) {
                self.defer(id);
                return;
            }
            // 前の解放が失敗で止まっている。再試行して、済まなければ開始しない（契約表 9）。
            self.advance();
            match &self.closing {
                None => {}
                Some(s) if s.op.is_some() => {
                    self.defer(id);
                    return;
                }
                Some(_) => {
                    deliver_event(id, PreviewEvent::StartFailed(StartFailure::Other));
                    return;
                }
            }
        }
        self.begin(id);
    }

    fn begin(&mut self, id: u64) {
        let Ok(capture) = MediaCapture::new() else {
            deliver_event(id, PreviewEvent::StartFailed(StartFailure::Other));
            return;
        };
        let mut s = new_session(id, capture);
        let op = spawn(
            DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::VideoCapture),
            &self.tx,
            move |r| Cmd::Devices(id, r),
        );
        match op {
            Ok(op) => {
                s.op = Some(op);
                self.current = Some(s);
            }
            Err(_) => {
                self.close(s);
                deliver_event(id, PreviewEvent::StartFailed(StartFailure::Other));
            }
        }
    }

    fn initialize(&mut self, id: u64, found: windows::core::Result<DeviceInformationCollection>) {
        let tx = self.tx.clone();
        let Some(s) = self.cur(id) else { return };
        s.op = None;
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
                &tx,
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

    fn after_init(&mut self, id: u64, result: windows::core::Result<()>) {
        let tx = self.tx.clone();
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
                &tx,
                move |r| Cmd::Reader(id, r),
            )
        })();
        match next {
            Ok(op) => s.op = Some(op),
            // ソースが無い（空エラー code 0）はカメラなし。
            Err(e) if e.code().0 == 0 => {
                self.end(id, PreviewEvent::StartFailed(StartFailure::NoCamera))
            }
            Err(e) => {
                let f = start_failure_from_windows_hresult(e.code().0);
                self.end(id, PreviewEvent::StartFailed(f));
            }
        }
    }

    fn after_reader(&mut self, id: u64, result: windows::core::Result<MediaFrameReader>) {
        let tx = self.tx.clone();
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
        // StartAsync を出すまでは Close だけでよい（始まっていない reader に StopAsync を出さない）。
        s.reader = Some(ReaderSlot::NeedsClose(reader.clone()));
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
            spawn(reader.StartAsync(), &tx, move |r| Cmd::ReaderStarted(id, r))
        })();
        match next {
            Ok(op) => {
                s.op = Some(op);
                s.reader = Some(ReaderSlot::NeedsStop(reader));
            }
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
            .and_then(|z| z.Supported().map(|sup| sup.then_some(z)))
            .map_err(|_| ());
        s.started = true;
        deliver_event(id, PreviewEvent::Running);
        // 開始確定前に届いた状態通知は無視されているため、現在の状態で一度評価し直す。
        self.stream_state(id);
    }

    /// `CameraStreamStateChanged` を受けた評価。状態の取得失敗と `Shutdown` は実行中の失敗になる。
    fn stream_state(&mut self, id: u64) {
        let Some(s) = self.cur(id) else { return };
        // 状態が取れないのに無視すると失敗を見逃すため、実行中の失敗として確定する（契約表 6）。
        let Ok(state) = s.capture.CameraStreamState() else {
            self.end(id, PreviewEvent::RuntimeFailed);
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
        let Some(reader) = s.reader_ref() else { return };
        // null・取得失敗は、そのフレームを捨てて続ける（フレームの間隔の欠けとして扱う）。
        let Ok(fr) = reader.TryAcquireLatestFrame() else {
            return;
        };
        let zoom = match &s.zoom {
            Ok(Some(z)) => z.Value().ok().and_then(checked_zoom),
            _ => None,
        };
        let bmp = fr.VideoMediaFrame().and_then(|v| v.SoftwareBitmap());
        let (frame, converted) = match &bmp {
            Ok(b) => convert(b, zoom),
            Err(_) => (None, None),
        };
        // 閉じないと供給が止まる。失敗は個別に再試行せず、実行中の失敗として session ごと解放する。
        let mut close_failed = fr.Close().is_err();
        if let Ok(b) = &bmp {
            close_failed |= b.Close().is_err();
        }
        if let Some(c) = &converted {
            close_failed |= c.Close().is_err();
        }
        if close_failed {
            self.end(id, PreviewEvent::RuntimeFailed);
            return;
        }
        if let Some(f) = frame {
            deliver_frame(id, f);
        }
    }
}

fn new_session(id: u64, capture: MediaCapture) -> Session {
    Session {
        id,
        capture,
        reader: None,
        op: None,
        failed_token: None,
        state_token: None,
        frame_token: None,
        frame_flag: Arc::new(AtomicBool::new(false)),
        started: false,
        interrupted: false,
        zoom: Ok(None),
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
    let z = match &s.zoom {
        Err(()) => return Err(ZoomFailure::Failed),
        Ok(None) => return Err(ZoomFailure::Unsupported),
        Ok(Some(z)) => z,
    };
    let range = z.Min().ok().zip(z.Max().ok());
    let target = zoom_target(step, range)?;
    let settings = ZoomSettings::new().map_err(|_| ZoomFailure::Failed)?;
    settings
        .SetMode(ZoomTransitionMode::Auto)
        .and_then(|_| settings.SetValue(target))
        .and_then(|_| z.Configure(&settings))
        .map_err(|_| ZoomFailure::Failed)
}

/// `SoftwareBitmap` を縮小済みの `PreviewFrame` にする。`None` は変換・読み出しの失敗（捨てて続ける）。
/// 2 つ目は変換で作った `SoftwareBitmap`で、呼び出し側が `Close` する。
fn convert(
    bmp: &SoftwareBitmap,
    zoom: Option<f32>,
) -> (Option<PreviewFrame>, Option<SoftwareBitmap>) {
    let converted = if bmp.BitmapPixelFormat().ok() == Some(BitmapPixelFormat::Bgra8) {
        None
    } else {
        match SoftwareBitmap::Convert(bmp, BitmapPixelFormat::Bgra8) {
            Ok(c) => Some(c),
            Err(_) => return (None, None),
        }
    };
    let frame = read_bgra(converted.as_ref().unwrap_or(bmp)).and_then(|(w, h, data)| {
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
            zoom,
        })
    });
    (frame, converted)
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
