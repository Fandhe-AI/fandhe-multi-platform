//! Windows 向け画面録画実装（Issue #26）。`device/screen_capture.rs` の dispatch から
//! 呼ばれ、`Windows.Graphics.Capture` で撮った画面を WinRT `MediaTranscoder` で
//! H.264 / MP4 へ符号化し、[`ScreenCapturer`] 実装（[`WindowsScreenCapturer`]）として
//! バイト数・mime・録画時間だけを `screen_capture::bridge` へ返す（映像は Rust へ読み込まない）。
//! 契約（三値・守る範囲）の SSOT は `fandhe_device::screen_capture` のモジュール doc。
//!
//! # エンコード経路の選定
//!
//! `MediaStreamSource` の `SampleRequested` で `MediaStreamSample::CreateFromDirect3D11Surface`
//! を渡すだけで `MediaTranscoder` が MP4 を作れる WinRT 経路を採る。Media Foundation の
//! `IMFSinkWriter` は `MFStartup`・`IMFSample`・`IMFMediaType` の組み立てがすべて unsafe COM に
//! なり、`unsafe` の範囲が数倍に広がるため採らない。出力は `InMemoryRandomAccessStream` で、
//! 一時ファイルを作らない（契約表 9 の削除分岐が要らない）。結果は `Size()` だけを読む。
//!
//! # 録画対象は毎回ピッカーで選ばせる
//!
//! `GraphicsCapturePicker`（`windows_share::window()` の HWND で初期化）を毎回出す。
//! プログラムからモニターを黙って取る `CreateForMonitor` は使わない。ピッカーを閉じた
//! （項目が null）・選択前に停止が押された場合は `Cancelled`。
//!
//! # スレッドと状態
//!
//! [`WindowsScreenCapturer::start`] は `screen_capture::drive_callback`（UI スレッド）から呼ばれ、
//! ピッカーを出して完了ハンドラを登録したら即戻る（`IAsyncOperation` は [`HELD`] に保持、
//! `windows_biometric.rs` と同型）。項目が得られたらワーカースレッドが D3D11・フレームプール・
//! セッション・トランスコードを生成から解放まで受け持つ。状態は [`SESSION`] 1 本で、
//! 「現在の要求か」の判定と更新は同じロックの内側で行い（契約表 2）、結果の配送前に
//! 自分の要求のときだけ解除する（解除が遅れると次の開始が busy になる）。
//!
//! # 結果の三値
//!
//! - 成功: 明示停止・10 秒の自動停止・対象ウィンドウが閉じた場合。書き出し完了
//!   （`AsyncStatus::Completed`）を確認し、0 バイト・0 ms は `build_capture_result` で失敗
//! - キャンセル: ピッカーを閉じた・ピッカー表示中の停止
//! - 失敗: `E_ACCESSDENIED` は `PermissionDenied`、`IsSupported()` が false は `Unsupported`、
//!   それ以外（D3D11・変換・トランスコード・完了待ちタイムアウト）は固定ラベルの `Internal`
//!
//! 録画時間は単調時計 `Instant` で測る（契約表 3）。HRESULT の生値・ウィンドウ名・映像の
//! 中身はログへ出さない（security.md A09）。
//!
//! # 既知の天井（守らない範囲）
//!
//! ピッカーをプログラムから閉じられない（表示中の停止はピッカー完了後に反映）、録画中の
//! サイズ変更に追従しない（`Recreate` しない）、エンコーダが解像度を拒否すれば失敗、
//! 静止画面ではフレームが少ない、キャプチャ枠の有無は OS 依存、複数プロセス・プロセス死。
//!
//! `unsafe` は D3D11 デバイス生成・サーフェス変換・テクスチャコピー・ピッカーの HWND
//! 初期化の 4 関数に閉じる（`// SAFETY:` 付き）。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCapturePicker, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::{IDirect3DDevice, IDirect3DSurface};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Media::Core::{
    MediaStreamSample, MediaStreamSource, MediaStreamSourceSampleRequestedEventArgs,
    VideoStreamDescriptor,
};
use windows::Media::MediaProperties::{
    MediaEncodingProfile, MediaEncodingSubtypes, VideoEncodingProperties, VideoEncodingQuality,
};
use windows::Media::Transcoding::MediaTranscoder;
use windows::Storage::Streams::InMemoryRandomAccessStream;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, CreateDirect3D11SurfaceFromDXGISurface,
    IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::UI::Shell::IInitializeWithWindow;
// `IAsyncOperation` 等は `windows::Foundation` へ再エクスポートされない（windows_biometric.rs 参照）。
use windows_future::{AsyncActionWithProgressCompletedHandler, AsyncOperationCompletedHandler};
use windows_future::{AsyncStatus, IAsyncOperation};

use fandhe_device::screen_capture::{build_capture_result, CaptureResult, ScreenCapturer};
use fandhe_device::DeviceError;

use super::screen_capture::{bridge, error_from_hresult, even_capture_size};

/// ワーカーがトランスコード完了を待つ猶予。超えたら `Cancel` して失敗にする。
const FINISH_GRACE: Duration = Duration::from_secs(5);

/// 進行中の録画 1 件。`request_id` 照合と状態遷移は必ず [`SESSION`] のロック内で行う。
struct Session {
    request_id: u64,
    phase: Phase,
}

enum Phase {
    /// ピッカー表示中。`stop_requested` はピッカー完了時に録画を始めず `Cancelled` にする合図。
    Picking { stop_requested: bool },
    /// ワーカーが録画中。`stop_tx` へ送ると停止する。
    Recording { stop_tx: Sender<()> },
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

thread_local! {
    /// 直近の `start` が発行したピッカーの `IAsyncOperation`（完了ハンドラが呼ばれる前に
    /// ドロップされないよう保持する。`start` は UI スレッドからのみ呼ばれる契約、
    /// `windows_biometric.rs::HELD` と同じ）。
    static HELD: RefCell<Option<IAsyncOperation<GraphicsCaptureItem>>> =
        const { RefCell::new(None) };
}

/// 内部エンジン（ゼロサイズ。状態は [`SESSION`] が持つ）。
struct WindowsScreenCapturer;

static CAPTURER: WindowsScreenCapturer = WindowsScreenCapturer;

/// OS 別エンジンを返す。
pub(crate) fn capturer() -> &'static dyn ScreenCapturer {
    &CAPTURER
}

fn lock_session() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

/// 自分の要求のときだけ [`SESSION`] を解除してから結果を配送する。
fn finish(request_id: u64, result: Result<CaptureResult, DeviceError>) {
    {
        let mut guard = lock_session();
        if guard.as_ref().is_some_and(|s| s.request_id == request_id) {
            *guard = None;
        }
    }
    bridge::deliver(request_id, result);
}

fn hr(e: &windows::core::Error, stage: &'static str) -> DeviceError {
    error_from_hresult(e.code().0, stage)
}

impl ScreenCapturer for WindowsScreenCapturer {
    fn start(&self, request_id: u64, max_duration: Duration) -> Result<(), DeviceError> {
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            return Err(DeviceError::Unsupported("screen_capture"));
        }
        let hwnd =
            super::windows_share::window().ok_or(DeviceError::Internal("screen_capture window"))?;
        {
            let mut guard = lock_session();
            if guard.is_some() {
                return Err(DeviceError::Internal("screen_capture busy"));
            }
            *guard = Some(Session {
                request_id,
                phase: Phase::Picking {
                    stop_requested: false,
                },
            });
        }
        // 同期失敗時は SESSION を戻して呼び出し元に `Err` を返す（完了ハンドラは来ない）。
        let started = begin_pick(request_id, max_duration, hwnd);
        if started.is_err() {
            let mut guard = lock_session();
            if guard.as_ref().is_some_and(|s| s.request_id == request_id) {
                *guard = None;
            }
        }
        started
    }

    fn stop(&self, request_id: u64) -> Result<(), DeviceError> {
        let mut guard = lock_session();
        if let Some(s) = guard.as_mut().filter(|s| s.request_id == request_id) {
            match &mut s.phase {
                Phase::Picking { stop_requested } => *stop_requested = true,
                Phase::Recording { stop_tx } => {
                    let _ = stop_tx.send(());
                }
            }
        }
        // 不一致・終了済みは無視（トレイト doc）。
        Ok(())
    }
}

/// ピッカーを出して完了ハンドラを登録する。
fn begin_pick(request_id: u64, max: Duration, hwnd: HWND) -> Result<(), DeviceError> {
    let picker = GraphicsCapturePicker::new().map_err(|e| hr(&e, "screen_capture picker"))?;
    init_picker_window(&picker, hwnd).map_err(|e| hr(&e, "screen_capture picker window"))?;
    let op = picker
        .PickSingleItemAsync()
        .map_err(|e| hr(&e, "screen_capture pick"))?;
    op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
        let item = match (status, op.ok()) {
            (AsyncStatus::Completed, Ok(op)) => Some(op.GetResults().ok()),
            (AsyncStatus::Error, _) => None,
            // Canceled・未知はキャンセル扱い。
            _ => Some(None),
        };
        on_picked(request_id, max, item);
        Ok(())
    }))
    .map_err(|e| hr(&e, "screen_capture pick completed"))?;
    HELD.with_borrow_mut(|held| *held = Some(op));
    Ok(())
}

/// `item`: `None` = ピッカーが失敗、`Some(None)` = 閉じた（null）、`Some(Some(_))` = 選択済み。
fn on_picked(request_id: u64, max: Duration, item: Option<Option<GraphicsCaptureItem>>) {
    let (tx, rx) = mpsc::channel::<()>();
    let item = {
        let mut guard = lock_session();
        let Some(s) = guard.as_mut().filter(|s| s.request_id == request_id) else {
            return; // 古い世代。
        };
        let stop_requested = matches!(
            s.phase,
            Phase::Picking {
                stop_requested: true
            }
        );
        match item {
            Some(Some(item)) if !stop_requested => {
                s.phase = Phase::Recording {
                    stop_tx: tx.clone(),
                };
                item
            }
            other => {
                *guard = None;
                drop(guard);
                let err = match other {
                    None => DeviceError::Internal("screen_capture pick"),
                    Some(_) => DeviceError::Cancelled("screen_capture"),
                };
                bridge::deliver(request_id, Err(err));
                return;
            }
        }
    };
    let spawned = std::thread::Builder::new()
        .name("fandhe-screen-capture".into())
        .spawn(move || {
            let result = record(item, request_id, max, &rx, tx);
            finish(request_id, result);
        });
    if spawned.is_err() {
        finish(
            request_id,
            Err(DeviceError::Internal("screen_capture spawn")),
        );
    }
}

/// 全終了経路でセッション・プール・`Closed` 登録を解放する（契約表 9）。
struct Cleanup {
    session: Option<GraphicsCaptureSession>,
    pool: Direct3D11CaptureFramePool,
    item: GraphicsCaptureItem,
    closed_token: Option<i64>,
    /// 失敗経路でもトランスコーダ側の再試行ループを止めるため、破棄時に立てる。
    stopping: Arc<AtomicBool>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(t) = self.closed_token {
            let _ = self.item.RemoveClosed(t);
        }
        if let Some(s) = &self.session {
            let _ = s.Close();
        }
        let _ = self.pool.Close();
    }
}

/// ワーカー本体。D3D11 生成からトランスコード完了待ちまでを受け持つ。
fn record(
    item: GraphicsCaptureItem,
    request_id: u64,
    max: Duration,
    stop_rx: &Receiver<()>,
    stop_tx: Sender<()>,
) -> Result<CaptureResult, DeviceError> {
    let size = item.Size().map_err(|e| hr(&e, "screen_capture item"))?;
    let (w, h) = even_capture_size(size.Width, size.Height)?;
    let (d3d, ctx, device) = create_devices().map_err(|e| hr(&e, "screen_capture d3d"))?;

    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        windows::Graphics::SizeInt32 {
            Width: w as i32,
            Height: h as i32,
        },
    )
    .map_err(|e| hr(&e, "screen_capture pool"))?;
    let stopping = Arc::new(AtomicBool::new(false));
    let mut cleanup = Cleanup {
        session: None,
        pool: pool.clone(),
        item: item.clone(),
        closed_token: None,
        stopping: stopping.clone(),
    };
    let session = pool
        .CreateCaptureSession(&item)
        .map_err(|e| hr(&e, "screen_capture session"))?;
    cleanup.session = Some(session.clone());

    // 対象ウィンドウが閉じたら停止合図を送る（成功扱いで書き出しを閉じる）。
    cleanup.closed_token = item
        .Closed(&TypedEventHandler::new(move |_, _| {
            let _ = stop_tx.send(());
            Ok(())
        }))
        .ok();

    let source = build_source(&pool, &d3d, &ctx, &stopping, w, h)?;

    let stream = InMemoryRandomAccessStream::new().map_err(|e| hr(&e, "screen_capture stream"))?;
    let profile = MediaEncodingProfile::CreateMp4(VideoEncodingQuality::HD720p)
        .map_err(|e| hr(&e, "screen_capture profile"))?;
    let video = profile
        .Video()
        .map_err(|e| hr(&e, "screen_capture profile"))?;
    video
        .SetWidth(w)
        .and_then(|()| video.SetHeight(h))
        .map_err(|e| hr(&e, "screen_capture profile"))?;
    let transcoder = MediaTranscoder::new().map_err(|e| hr(&e, "screen_capture transcoder"))?;
    let prepared = transcoder
        .PrepareMediaStreamSourceTranscodeAsync(&source, &stream, &profile)
        .and_then(|op| op.get())
        .map_err(|e| hr(&e, "screen_capture prepare"))?;
    if !prepared.CanTranscode().unwrap_or(false) {
        return Err(DeviceError::Internal("screen_capture transcode"));
    }

    let (done_tx, done_rx) = mpsc::channel::<AsyncStatus>();
    session
        .StartCapture()
        .map_err(|e| hr(&e, "screen_capture start"))?;
    let action = prepared
        .TranscodeAsync()
        .map_err(|e| hr(&e, "screen_capture transcode"))?;
    action
        .SetCompleted(&AsyncActionWithProgressCompletedHandler::new(
            move |_, status| {
                let _ = done_tx.send(status);
                Ok(())
            },
        ))
        .map_err(|e| {
            let _ = action.Cancel();
            hr(&e, "screen_capture transcode")
        })?;
    bridge::mark_started(request_id);
    let t0 = Instant::now();

    // 停止合図・期限・`Closed` のどれでも（送信側が全部落ちても）ここで抜ける。
    let _ = stop_rx.recv_timeout(max);
    let elapsed_ms = t0.elapsed().as_millis() as u64;
    stopping.store(true, Ordering::SeqCst);

    match done_rx.recv_timeout(FINISH_GRACE) {
        Ok(AsyncStatus::Completed) => {}
        Ok(_) => return Err(DeviceError::Internal("screen_capture transcode")),
        Err(_) => {
            let _ = action.Cancel();
            return Err(DeviceError::Internal("screen_capture finish timeout"));
        }
    }
    let bytes = stream.Size().map_err(|e| hr(&e, "screen_capture size"))?;
    drop(cleanup);
    build_capture_result(bytes, "video/mp4".to_string(), elapsed_ms)
}

/// フレームプールから 1 枚ずつ取り出してサンプル化する `MediaStreamSource` を作る。
/// `stopping` が立つとサンプルを空にして EOS を送る。
fn build_source(
    pool: &Direct3D11CaptureFramePool,
    d3d: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    stopping: &Arc<AtomicBool>,
    w: u32,
    h: u32,
) -> Result<MediaStreamSource, DeviceError> {
    let err = |e: windows::core::Error| hr(&e, "screen_capture source");
    let subtype = MediaEncodingSubtypes::Bgra8().map_err(err)?;
    let props = VideoEncodingProperties::CreateUncompressed(&subtype, w, h).map_err(err)?;
    let descriptor = VideoStreamDescriptor::Create(&props).map_err(err)?;
    let source = MediaStreamSource::CreateFromDescriptor(&descriptor).map_err(err)?;
    source
        .SetBufferTime(TimeSpan { Duration: 0 })
        .map_err(err)?;

    let (pool, d3d, ctx, stopping) = (pool.clone(), d3d.clone(), ctx.clone(), stopping.clone());
    let mut first: Option<i64> = None;
    source
        .SampleRequested(&TypedEventHandler::<
            MediaStreamSource,
            MediaStreamSourceSampleRequestedEventArgs,
        >::new(move |_, args| {
            let request = args.ok()?.Request()?;
            let frame = loop {
                if stopping.load(Ordering::SeqCst) {
                    return Ok(()); // サンプル無し = EOS
                }
                match pool.TryGetNextFrame() {
                    Ok(f) => break f,
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            };
            // タイムスタンプは最初のフレームとの差（単調）。
            let now = frame.SystemRelativeTime()?.Duration;
            let base = *first.get_or_insert(now);
            // プールへ返した面をエンコーダが読まないよう、自前のテクスチャへコピーする。
            let surface = copy_frame_surface(&d3d, &ctx, &frame.Surface()?)?;
            let sample = MediaStreamSample::CreateFromDirect3D11Surface(
                &surface,
                TimeSpan {
                    Duration: now - base,
                },
            )?;
            request.SetSample(&sample)?;
            let _ = frame.Close();
            Ok(())
        }))
        .map_err(err)?;
    Ok(source)
}

#[allow(unsafe_code)]
fn init_picker_window(picker: &GraphicsCapturePicker, hwnd: HWND) -> windows::core::Result<()> {
    let init: IInitializeWithWindow = picker.cast()?;
    // SAFETY: `hwnd` は `windows_share::window()`（`remember_window` が控えた有効なウィンドウ）
    // 由来で、`init` は直前に取得した有効な COM ポインタ（`windows_biometric.rs` と同じ根拠）。
    unsafe { init.Initialize(hwnd) }
}

#[allow(unsafe_code)]
fn create_devices() -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext, IDirect3DDevice)> {
    let mut d3d: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    // SAFETY: 出力先は上で用意した有効な `Option` で、アダプタ・機能レベル配列は未指定
    // （既定）。戻り後に `Option` が `Some` かを確認してから使う。
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d),
            None,
            Some(&mut ctx),
        )?;
    }
    let (d3d, ctx) =
        d3d.zip(ctx)
            .ok_or(windows::core::Error::from_hresult(windows::core::HRESULT(
                0x8000_4005_u32 as i32,
            )))?;
    // エンコーダ（別スレッド）と同じデバイスを触るため、スレッド保護を有効にする。
    if let Ok(mt) = d3d.cast::<ID3D11Multithread>() {
        // SAFETY: 有効な COM ポインタへの単純な設定呼び出し。
        #[allow(unsafe_code)]
        let _ = unsafe { mt.SetMultithreadProtected(true) };
    }
    let dxgi: IDXGIDevice = d3d.cast()?;
    // SAFETY: `dxgi` は直前に取得した有効な `IDXGIDevice`。
    let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
    Ok((d3d, ctx, winrt))
}

#[allow(unsafe_code)]
fn copy_frame_surface(
    d3d: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    src: &IDirect3DSurface,
) -> windows::core::Result<IDirect3DSurface> {
    let access: IDirect3DDxgiInterfaceAccess = src.cast()?;
    // SAFETY: `access` は `src`（フレームの有効なサーフェス）から得た COM ポインタで、
    // 要求する型 `ID3D11Texture2D` はキャプチャのバッキングテクスチャ。
    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` は有効な出力先。
    unsafe { texture.GetDesc(&mut desc) };
    desc.BindFlags = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
    desc.MiscFlags = 0;
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.CPUAccessFlags = 0;
    let mut copy: Option<ID3D11Texture2D> = None;
    // SAFETY: `desc` は元テクスチャ由来の有効な記述で、初期データは渡さない。
    unsafe { d3d.CreateTexture2D(&desc, None, Some(&mut copy))? };
    let copy = copy.ok_or(windows::core::Error::from_hresult(windows::core::HRESULT(
        0x8000_4005_u32 as i32,
    )))?;
    // SAFETY: 同じデバイスが作った同サイズ・同形式の 2 テクスチャ（マルチスレッド保護済み）。
    unsafe { ctx.CopyResource(&copy, &texture) };
    let dxgi: IDXGISurface = copy.cast()?;
    // SAFETY: `dxgi` は直前に取得した有効な `IDXGISurface`。
    unsafe { CreateDirect3D11SurfaceFromDXGISurface(&dxgi)? }.cast()
}
