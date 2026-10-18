//! Windows 向け外部機器への再生（Issue #388、親 #99 の windows 子）。`device/media_cast.rs::platform_cast` の
//! Windows 経路から呼ばれ、契約（SSOT）は `fandhe_device::media_cast`。
//! 実行時に生成した WAV を WinRT `MediaPlayer`（メモリ上の `InMemoryRandomAccessStream`、ループ）で鳴らし、
//! アプリのボタンから OS の `CastingDevicePicker` を出して Miracast・DLNA・Bluetooth の受信機へ送る。
//! 出力先の名前と接続状態は画面に出す。画面ミラーリング・動画・アプリからの切断は範囲外。
//!
//! # 方式と、スタブ時の注意との違い
//!
//! `CastingSource` は `MediaPlayer::GetAsCastingSource` からしか得られないため、Media Engine
//! （`windows_media_playback.rs`）は流用しない。WinRT の活性化は実行時にだけ失敗し import table に載らない
//! ので、Windows N でも exe の起動は巻き込まない（活性化失敗は `Failed(Unsupported)`）。
//! スタブの doc は「接続は UI スレッドで」としていたが、デスクトップには CoreDispatcher がなく
//! `CastingDevice` / `CastingConnection` / `MediaPlayer` は agile のため、接続はワーカーで行う
//! （実機で確認する項目、#379）。ピッカーだけは owner window の都合で UI スレッドで開く。
//!
//! # スレッドと直列化（契約表 2・11）
//!
//! 状態は純ロジックの [`Shared`]（1 本の `Mutex`）が持ち、判定・更新・キュー送信は同じ排他区間で行う。
//! OS の資源（`MediaPlayer`・`CastingConnection`・イベントトークン）は MTA ワーカー 1 本が `mpsc` の直列
//! キューで扱う。OS のコールバックは世代付きでキューへ戻すだけで、中でロックも OS 呼び出しもしない。
//! 到着順の入れ替わりは保持参照ではなく実際の `route` を見て判断する（`Dismissed` が `Selected` より
//! 後に届いても `Connecting` を戻さない）。後から届く通知は世代照合で捨てる。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: `Playing` は OS の再生中通知かつ意図が再生のときだけ。`Connected` は `StateChanged` の
//!   Connected / Rendering を受けたときだけ
//! - キャンセル: ピッカーを閉じたら `Choosing` → `Local`（失敗にしない）。停止は `Stopped`
//! - 失敗: 開始結果・`MediaFailed` は [`failure_from_start_status`] / [`failure_from_media_error`] の
//!   1 か所で写し、生コードと出力先の名前はログ・エラー文に出さない（A09）
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の操作順、失敗を成功値へ畳まないこと、ハンドラを外してから閉じる資源の解放、
//! 古い世代の通知を捨てること。
//! 守らない: ポーリング間隔（約 250ms）ぶんの反映遅れ、OS がピッカーの閉鎖を通知するまでの遅れ、
//! 複数窓・複数プロセス、受信機側の品質、`NoDevices` の検出（ピッカーが自前で「見つからない」を出す）、
//! UI スレッド以外から `release` したときのピッカーの後始末（プロセス終了に任せる）。
//!
//! # unsafe
//!
//! `IInitializeWithWindow::Initialize` の 1 か所だけ（`win::init_picker_window`）。

#[cfg(target_os = "windows")]
use std::sync::Arc;

use fandhe_device::device_info::clean_text;
use fandhe_device::media_cast::{CastFailure, CastPlayback, CastSnapshot, PickerKind, RouteState};
use fandhe_device::DeviceError;

/// `prepare` に渡せる WAV の上限（他の再生実装と同じ。異常に大きい入力を OS に渡さない）。
const MAX_WAV_BYTES: usize = 16 * 1024 * 1024;

/// 画面（ドライバ）が最後に求めた再生操作。遅れて届く通知を捨てる判断に使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intent {
    Play,
    Stop,
}

/// OS の通知を、世代照合前の形で表したもの。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ev {
    /// `MediaOpened`。
    Opened,
    /// `PlaybackStateChanged` が Playing。
    Playing,
    /// `PlaybackStateChanged` が Paused。
    Paused,
    /// `MediaFailed`（写像済み）。
    MediaFailed(CastFailure),
    /// ピッカーで出力先が選ばれた（接続の連番, `FriendlyName` の生文字列）。
    Selected(u64, String),
    /// ピッカーが閉じた。
    Dismissed,
    /// `CastingConnection.StateChanged`（接続の連番, `CastingConnectionState` の値）。
    /// 連番が現在の接続と違えば入れ替え前の旧接続の通知なので捨てる。
    ConnState(u64, i32),
    /// `CastingConnection.ErrorOccurred`（接続の連番, `CastingConnectionErrorStatus` の値）。
    ConnError(u64, i32),
    /// `RequestStartCastingAsync` の完了（接続の連番, `CastingConnectionErrorStatus` の値）。
    /// 連番が現在の接続と違えば、入れ替え前の旧接続の完了なので捨てる。
    StartResult(u64, i32),
}

/// `CastingConnectionErrorStatus` を契約の失敗へ写す。0（Succeeded）は `None`。写像はここ 1 か所。
/// 3 = DeviceLocked、5 = InvalidCastingSource（受信機がメモリ上の音源を受けない場合の受け皿）。
fn failure_from_start_status(code: i32) -> Option<CastFailure> {
    match code {
        0 => None,
        3 => Some(CastFailure::Blocked),
        5 => Some(CastFailure::SourceNotSupported),
        _ => Some(CastFailure::Os),
    }
}

/// `MediaPlayerError` を契約の失敗へ写す。3 = DecodingError、4 = SourceNotSupported。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn failure_from_media_error(code: i32) -> CastFailure {
    match code {
        3 | 4 => CastFailure::SourceNotSupported,
        _ => CastFailure::Os,
    }
}

/// 世代・状態・意図を 1 本のロックで守る純ロジック（host でテストする）。
struct Shared {
    next_gen: u64,
    current: Option<u64>,
    playback: CastPlayback,
    route: RouteState,
    name: Option<String>,
    intent: Intent,
    /// 現在の接続の連番（`Selected` で進む）。0 は接続なし。世代と違い 1 世代の中で接続を入れ替えるたびに進む。
    conn_seq: u64,
    /// 再生開始の通知前に停止を要求した（`Paused` を `Stopped` へ確定する印）。
    pending_stop: bool,
}

impl Shared {
    const fn new() -> Self {
        Self {
            next_gen: 0,
            current: None,
            playback: CastPlayback::Idle,
            route: RouteState::Local,
            name: None,
            intent: Intent::Stop,
            conn_seq: 0,
            pending_stop: false,
        }
    }

    /// 以後の古い世代の操作と通知を捨てる状態へ戻す（`release` 用）。`next_gen` は戻さない（ABA 防止）。
    fn reset(&mut self) {
        self.current = None;
        self.playback = CastPlayback::Idle;
        self.route = RouteState::Local;
        self.name = None;
        self.intent = Intent::Stop;
        self.conn_seq = 0;
        self.pending_stop = false;
    }

    /// 新しい準備を始める。0 バイトと上限超は拒み、旧世代も無効にする。
    fn begin_prepare(&mut self, len: usize) -> Result<u64, DeviceError> {
        self.reset();
        if len == 0 || len > MAX_WAV_BYTES {
            return Err(DeviceError::InvalidInput("media_cast wav"));
        }
        self.next_gen += 1;
        self.current = Some(self.next_gen);
        Ok(self.next_gen)
    }

    fn check(&self, gen: u64) -> Result<(), DeviceError> {
        if self.current == Some(gen) {
            Ok(())
        } else {
            Err(DeviceError::InvalidInput("media_cast generation"))
        }
    }

    /// 意図だけ記録する。`Playing` は OS の通知で立てる。
    fn on_play(&mut self, gen: u64) -> Result<(), DeviceError> {
        self.check(gen)?;
        if matches!(self.playback, CastPlayback::Failed(_)) {
            return Err(DeviceError::InvalidInput("media_cast failed"));
        }
        self.intent = Intent::Play;
        self.pending_stop = false;
        Ok(())
    }

    /// 戻り値は OS へ停止を流すべきか。再生中か、`play` 済みで Playing 通知が未達（キュー内の
    /// `Cmd::Play` が後で鳴らしうる）なら流す。Stop は冪等。`Stopped` は Paused の通知で立てる。
    fn on_stop(&mut self, gen: u64) -> Result<bool, DeviceError> {
        self.check(gen)?;
        let was_play = self.intent == Intent::Play;
        self.intent = Intent::Stop;
        let flush = was_play || self.playback == CastPlayback::Playing;
        // Playing 通知より前の停止でも、後から届く Paused で Stopped を確定する。
        self.pending_stop = flush && self.playback != CastPlayback::Playing;
        Ok(flush)
    }

    /// ピッカーを出す前の確認。`Choosing` は `Show` の前に立てる（閉じた通知が先に届いても戻せる）。
    /// `Local` のときだけ立てる。接続中・接続済み・切断のまま閉じられても `Local` へ戻さない（嘘の状態にしない）。
    fn on_picker_shown(&mut self, gen: u64) -> Result<(), DeviceError> {
        self.check(gen)?;
        if !matches!(
            self.playback,
            CastPlayback::Ready | CastPlayback::Playing | CastPlayback::Stopped
        ) {
            return Err(DeviceError::Internal("media_cast not ready"));
        }
        if self.route == RouteState::Local {
            self.route = RouteState::Choosing;
        }
        Ok(())
    }

    /// 接続の失敗を確定する。接続中のときだけ（遅れて届いた失敗で別状態を壊さない）。
    fn fail_connecting(&mut self, f: CastFailure) {
        if self.route == RouteState::Connecting {
            self.route = RouteState::Local;
            if !matches!(self.playback, CastPlayback::Failed(_)) {
                self.playback = CastPlayback::Failed(f);
            }
        }
    }

    /// 世代の違う通知は捨てる（契約表 2）。
    fn on_event(&mut self, gen: u64, ev: Ev) {
        if self.check(gen).is_err() {
            return;
        }
        let failed = matches!(self.playback, CastPlayback::Failed(_));
        match ev {
            Ev::Opened => {
                if self.playback == CastPlayback::Idle {
                    self.playback = CastPlayback::Ready;
                }
            }
            Ev::Playing => {
                if self.intent == Intent::Play && !failed {
                    self.playback = CastPlayback::Playing;
                }
            }
            Ev::Paused => {
                if self.intent == Intent::Stop
                    && (self.playback == CastPlayback::Playing
                        || (self.pending_stop && self.playback == CastPlayback::Ready))
                {
                    self.playback = CastPlayback::Stopped;
                    self.pending_stop = false;
                }
            }
            Ev::MediaFailed(f) => {
                if !failed {
                    self.playback = CastPlayback::Failed(f);
                }
            }
            // 閉じた通知が先に届いても、後続の選択で `Connecting` へ進める。
            Ev::Selected(seq, raw) => {
                if seq <= self.conn_seq {
                    return;
                }
                self.conn_seq = seq;
                self.route = RouteState::Connecting;
                self.name = clean_text(&raw);
            }
            Ev::Dismissed => {
                if self.route == RouteState::Choosing {
                    self.route = RouteState::Local;
                }
            }
            Ev::ConnState(seq, _) | Ev::ConnError(seq, _) if seq != self.conn_seq => {}
            Ev::ConnState(_, s) => {
                let connected = matches!(self.route, RouteState::Connected { .. });
                match s {
                    1 | 2 if connected || self.route == RouteState::Connecting => {
                        self.route = RouteState::Connected {
                            name: self.name.clone(),
                        };
                    }
                    4 if connected || self.route == RouteState::Disconnected => {
                        self.route = RouteState::Connecting;
                    }
                    0 | 3 if connected => self.route = RouteState::Disconnected,
                    _ => {}
                }
            }
            Ev::ConnError(_, code) => match self.route {
                RouteState::Connecting => {
                    self.fail_connecting(failure_from_start_status(code).unwrap_or(CastFailure::Os))
                }
                RouteState::Connected { .. } => self.route = RouteState::Disconnected,
                _ => {}
            },
            Ev::StartResult(seq, code) => {
                if seq != self.conn_seq {
                    return;
                }
                if let Some(f) = failure_from_start_status(code) {
                    self.fail_connecting(f);
                }
            }
        }
    }

    fn snapshot(&self) -> CastSnapshot {
        CastSnapshot {
            generation: self.current.unwrap_or(0),
            playback: self.playback,
            route: self.route.clone(),
            picker: PickerKind::App,
        }
    }
}

/// 現在の実装（Windows 以外では本モジュールは host テストにだけ含まれ、呼ばれない）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Arc<dyn fandhe_device::media_cast::MediaCast> {
    Arc::new(win::WindowsMediaCast)
}

#[cfg(target_os = "windows")]
mod win {
    use std::cell::RefCell;
    use std::sync::{mpsc, Mutex, OnceLock};

    use windows::core::{Interface, Ref, HSTRING};
    use windows::Foundation::{Rect, TimeSpan, TypedEventHandler};
    use windows::Media::Casting::{
        CastingConnection, CastingConnectionErrorOccurredEventArgs, CastingConnectionErrorStatus,
        CastingDevice, CastingDevicePicker, CastingDeviceSelectedEventArgs,
    };
    use windows::Media::Core::MediaSource;
    use windows::Media::Playback::{
        MediaPlaybackSession, MediaPlaybackState, MediaPlayer, MediaPlayerFailedEventArgs,
    };
    use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::IInitializeWithWindow;
    use windows_future::{AsyncOperationCompletedHandler, AsyncStatus, IAsyncOperation};

    use fandhe_device::media_cast::MediaCast;

    use super::*;
    use crate::device::windows_notification::ComInit;

    /// `REGDB_E_CLASSNOTREG`（WinRT クラスが活性化できない環境）。
    const CLASS_NOT_REG: i32 = 0x8004_0154_u32 as i32;

    static SHARED: Mutex<Shared> = Mutex::new(Shared::new());

    /// ロックの poison は無視して進める。
    fn with_shared<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
        f(&mut SHARED.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// 状態更新とキュー送信を同じ排他区間で行う（契約表 2・11）。`send` は待たないのでロック内で呼んでよい。
    fn with_shared_send(
        f: impl FnOnce(&mut Shared) -> Result<Cmd, DeviceError>,
    ) -> Result<(), DeviceError> {
        let mut s = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        let cmd = f(&mut s)?;
        send(cmd)
    }

    enum Cmd {
        Prepare(u64, Arc<[u8]>),
        Play(u64),
        Stop(u64),
        Selected(u64, CastingDevice),
        Event(u64, Ev),
        Release,
    }

    /// `None` はワーカーを立てられなかったことの記録（以後の `prepare` は失敗）。
    static QUEUE: OnceLock<Option<mpsc::Sender<Cmd>>> = OnceLock::new();

    fn queue() -> Option<&'static mpsc::Sender<Cmd>> {
        QUEUE
            .get_or_init(|| {
                let (tx, rx) = mpsc::channel();
                let tx2 = tx.clone();
                std::thread::Builder::new()
                    .name("fandhe-media-cast".into())
                    .spawn(move || worker(rx, tx2))
                    .ok()
                    .map(|_| tx)
            })
            .as_ref()
    }

    fn send(cmd: Cmd) -> Result<(), DeviceError> {
        match queue() {
            Some(q) if q.send(cmd).is_ok() => Ok(()),
            _ => Err(DeviceError::Internal("media_cast")),
        }
    }

    /// HRESULT を契約のエラー種別へ写す（写像はここ 1 か所、生コードは保持しない）。
    fn map_err(e: &windows::core::Error, stage: &'static str) -> DeviceError {
        if e.code().0 == CLASS_NOT_REG {
            DeviceError::Unsupported("media_cast")
        } else {
            DeviceError::Internal(stage)
        }
    }

    pub(super) struct WindowsMediaCast;

    impl MediaCast for WindowsMediaCast {
        fn prepare(&self, wav: Arc<[u8]>) -> Result<u64, DeviceError> {
            hide_picker();
            let mut gen = 0;
            let r = with_shared_send(|s| {
                gen = s.begin_prepare(wav.len())?;
                Ok(Cmd::Prepare(gen, wav))
            });
            if let Err(e) = r {
                // 旧世代の資源が残らないようにする（入力不正でも `begin_prepare` が無効化済み）。
                self.release();
                return Err(e);
            }
            Ok(gen)
        }

        fn play(&self, generation: u64) -> Result<(), DeviceError> {
            with_shared_send(|s| s.on_play(generation).map(|_| Cmd::Play(generation)))
        }

        fn stop(&self, generation: u64) -> Result<(), DeviceError> {
            let mut s = SHARED.lock().unwrap_or_else(|e| e.into_inner());
            if s.on_stop(generation)? {
                send(Cmd::Stop(generation))?;
            }
            Ok(())
        }

        fn show_picker(&self, generation: u64) -> Result<(), DeviceError> {
            with_shared(|s| s.check(generation))?;
            let hwnd = super::super::windows_share::window()
                .ok_or(DeviceError::Internal("media_cast window"))?;
            let tx = queue().ok_or(DeviceError::Internal("media_cast"))?.clone();
            with_shared(|s| s.on_picker_shown(generation))?;
            hide_picker();
            match open_picker(generation, hwnd, tx) {
                Ok(held) => {
                    HELD.with_borrow_mut(|h| *h = Some(held));
                    Ok(())
                }
                Err(e) => {
                    with_shared(|s| s.on_event(generation, Ev::Dismissed));
                    Err(map_err(&e, "media_cast picker"))
                }
            }
        }

        fn snapshot(&self) -> CastSnapshot {
            with_shared(|s| s.snapshot())
        }

        fn release(&self) {
            hide_picker();
            // reset と Release 送信を同じ排他区間に置く（間に別の prepare が入らない）。
            let mut s = SHARED.lock().unwrap_or_else(|e| e.into_inner());
            s.reset();
            if let Some(q) = QUEUE.get().and_then(|q| q.as_ref()) {
                let _ = q.send(Cmd::Release);
            }
        }
    }

    /// UI スレッドが持つピッカーとハンドラのトークン。
    struct Held {
        picker: CastingDevicePicker,
        selected: i64,
        dismissed: i64,
    }

    thread_local! {
        static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
    }

    /// 開いているピッカーを閉じ、ハンドラを外す（UI スレッド。他スレッドでは何もしない）。
    fn hide_picker() {
        if let Some(h) = HELD.with_borrow_mut(|h| h.take()) {
            let _ = h.picker.RemoveCastingDeviceSelected(h.selected);
            let _ = h.picker.RemoveCastingDevicePickerDismissed(h.dismissed);
            let _ = h.picker.Hide();
        }
    }

    fn open_picker(gen: u64, hwnd: HWND, tx: mpsc::Sender<Cmd>) -> windows::core::Result<Held> {
        let picker = CastingDevicePicker::new()?;
        picker.Filter()?.SetSupportsAudio(true)?;
        init_picker_window(&picker, hwnd)?;
        let tx_sel = tx.clone();
        let selected = picker.CastingDeviceSelected(&TypedEventHandler::new(
            move |_, args: Ref<'_, CastingDeviceSelectedEventArgs>| {
                if let Some(args) = args.as_ref() {
                    if let Ok(device) = args.SelectedCastingDevice() {
                        let _ = tx_sel.send(Cmd::Selected(gen, device));
                    }
                }
                Ok(())
            },
        ))?;
        let dismissed =
            match picker.CastingDevicePickerDismissed(&TypedEventHandler::new(move |_, _| {
                let _ = tx.send(Cmd::Event(gen, Ev::Dismissed));
                Ok(())
            })) {
                Ok(t) => t,
                Err(e) => {
                    let _ = picker.RemoveCastingDeviceSelected(selected);
                    return Err(e);
                }
            };
        // ponytail: 表示位置は窓に追従しない固定値。実機で位置が悪ければ GetClientRect で求める。
        let shown = picker.Show(Rect {
            X: 24.0,
            Y: 24.0,
            Width: 160.0,
            Height: 40.0,
        });
        if let Err(e) = shown {
            let _ = picker.RemoveCastingDeviceSelected(selected);
            let _ = picker.RemoveCastingDevicePickerDismissed(dismissed);
            return Err(e);
        }
        Ok(Held {
            picker,
            selected,
            dismissed,
        })
    }

    #[allow(unsafe_code)]
    fn init_picker_window(picker: &CastingDevicePicker, hwnd: HWND) -> windows::core::Result<()> {
        let init: IInitializeWithWindow = picker.cast()?;
        // SAFETY: `hwnd` は `windows_share::window()`（`remember_window` が控えた有効なウィンドウ）
        // 由来で、`init` は直前に取得した有効な COM ポインタ（`windows_screen_capture.rs` と同じ根拠）。
        unsafe { init.Initialize(hwnd) }
    }

    /// ワーカーが持つ 1 世代ぶんの OS 資源。
    struct Res {
        gen: u64,
        player: MediaPlayer,
        _stream: InMemoryRandomAccessStream,
        opened: i64,
        failed: i64,
        state: i64,
        conn: Option<Conn>,
    }

    struct Conn {
        conn: CastingConnection,
        state: i64,
        error: i64,
        /// 完了ハンドラが走る前に drop されてキャンセルされないよう保持する。
        _op: Option<IAsyncOperation<CastingConnectionErrorStatus>>,
    }

    /// ハンドラからキューへ通知を戻す。
    fn post(tx: &mpsc::Sender<Cmd>, gen: u64, ev: Ev) {
        let _ = tx.send(Cmd::Event(gen, ev));
    }

    fn create_player(gen: u64, wav: &[u8], tx: &mpsc::Sender<Cmd>) -> windows::core::Result<Res> {
        let stream = InMemoryRandomAccessStream::new()?;
        let writer = DataWriter::CreateDataWriter(&stream)?;
        writer.WriteBytes(wav)?;
        writer.StoreAsync()?.get()?;
        // drop で stream が閉じられないよう切り離す。
        writer.DetachStream()?;
        stream.Seek(0)?;
        let source = MediaSource::CreateFromStream(&stream, &HSTRING::from("audio/wav"))?;
        let player = MediaPlayer::new()?;
        // 以降の失敗経路ではハンドラを外してから閉じる（契約表 9）。
        let built = (|| {
            player.SetAutoPlay(false)?;
            player.SetIsLoopingEnabled(true)?;
            let t = tx.clone();
            let opened = player.MediaOpened(&TypedEventHandler::new(move |_, _| {
                post(&t, gen, Ev::Opened);
                Ok(())
            }))?;
            let t = tx.clone();
            let failed = match player.MediaFailed(&TypedEventHandler::new(
                move |_, args: Ref<'_, MediaPlayerFailedEventArgs>| {
                    let code = args
                        .as_ref()
                        .and_then(|a| a.Error().ok())
                        .map_or(0, |e| e.0);
                    post(&t, gen, Ev::MediaFailed(failure_from_media_error(code)));
                    Ok(())
                },
            )) {
                Ok(t) => t,
                Err(e) => {
                    let _ = player.RemoveMediaOpened(opened);
                    return Err(e);
                }
            };
            let t = tx.clone();
            let state = player.PlaybackSession().and_then(|session| {
                session.PlaybackStateChanged(&TypedEventHandler::new(
                    move |s: Ref<'_, MediaPlaybackSession>, _| {
                        if let Some(s) = s.as_ref() {
                            match s.PlaybackState() {
                                Ok(MediaPlaybackState::Playing) => post(&t, gen, Ev::Playing),
                                Ok(MediaPlaybackState::Paused) => post(&t, gen, Ev::Paused),
                                _ => {}
                            }
                        }
                        Ok(())
                    },
                ))
            });
            let state = match state {
                Ok(t) => t,
                Err(e) => {
                    let _ = player.RemoveMediaOpened(opened);
                    let _ = player.RemoveMediaFailed(failed);
                    return Err(e);
                }
            };
            player.SetSource(&source).inspect_err(|_| {
                let _ = player.RemoveMediaOpened(opened);
                let _ = player.RemoveMediaFailed(failed);
                if let Ok(session) = player.PlaybackSession() {
                    let _ = session.RemovePlaybackStateChanged(state);
                }
            })?;
            Ok((opened, failed, state))
        })();
        match built {
            Ok((opened, failed, state)) => Ok(Res {
                gen,
                player,
                _stream: stream,
                opened,
                failed,
                state,
                conn: None,
            }),
            Err(e) => {
                let _ = player.Close();
                Err(e)
            }
        }
    }

    /// 接続を閉じる。ハンドラを外してから `Close` する。
    fn close_conn(c: Conn) {
        let _ = c.conn.RemoveStateChanged(c.state);
        let _ = c.conn.RemoveErrorOccurred(c.error);
        let _ = c.conn.DisconnectAsync();
        let _ = c.conn.Close();
    }

    /// 資源を解放する。ハンドラを全部外してから接続とプレーヤーを閉じる。
    fn teardown(res: Res) {
        let _ = res.player.RemoveMediaOpened(res.opened);
        let _ = res.player.RemoveMediaFailed(res.failed);
        if let Ok(session) = res.player.PlaybackSession() {
            let _ = session.RemovePlaybackStateChanged(res.state);
        }
        if let Some(c) = res.conn {
            close_conn(c);
        }
        let _ = res.player.Close();
    }

    /// 選ばれた出力先へ接続を始める。結果は `Cmd::Event` で届く。
    fn connect(
        res: &mut Res,
        device: &CastingDevice,
        seq: u64,
        tx: &mpsc::Sender<Cmd>,
    ) -> windows::core::Result<()> {
        if let Some(old) = res.conn.take() {
            close_conn(old);
        }
        let gen = res.gen;
        let conn = device.CreateCastingConnection()?;
        let t = tx.clone();
        let state = conn.StateChanged(&TypedEventHandler::new(
            move |c: Ref<'_, CastingConnection>, _| {
                if let Some(c) = c.as_ref() {
                    if let Ok(s) = c.State() {
                        post(&t, gen, Ev::ConnState(seq, s.0));
                    }
                }
                Ok(())
            },
        ))?;
        let t = tx.clone();
        let error = match conn.ErrorOccurred(&TypedEventHandler::new(
            move |_, args: Ref<'_, CastingConnectionErrorOccurredEventArgs>| {
                let code = args
                    .as_ref()
                    .and_then(|a| a.ErrorStatus().ok())
                    .map_or(CastingConnectionErrorStatus::Unknown.0, |s| s.0);
                post(&t, gen, Ev::ConnError(seq, code));
                Ok(())
            },
        )) {
            Ok(t) => t,
            Err(e) => {
                let _ = conn.RemoveStateChanged(state);
                let _ = conn.Close();
                return Err(e);
            }
        };
        let mut c = Conn {
            conn,
            state,
            error,
            _op: None,
        };
        let op = match res
            .player
            .GetAsCastingSource()
            .and_then(|src| c.conn.RequestStartCastingAsync(&src))
        {
            Ok(op) => op,
            Err(e) => {
                close_conn(c);
                return Err(e);
            }
        };
        let t = tx.clone();
        let set = op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
            let code = match status {
                AsyncStatus::Completed => op.ok().and_then(|op| op.GetResults()).map_or(
                    CastingConnectionErrorStatus::Unknown.0,
                    |s: CastingConnectionErrorStatus| s.0,
                ),
                _ => CastingConnectionErrorStatus::Unknown.0,
            };
            post(&t, gen, Ev::StartResult(seq, code));
            Ok(())
        }));
        if let Err(e) = set {
            close_conn(c);
            return Err(e);
        }
        c._op = Some(op);
        res.conn = Some(c);
        Ok(())
    }

    fn fail(gen: u64, f: CastFailure) {
        with_shared(|s| s.on_event(gen, Ev::MediaFailed(f)));
    }

    fn worker(rx: mpsc::Receiver<Cmd>, tx: mpsc::Sender<Cmd>) {
        // 失敗しても worker は続け、以後の `Prepare` を `Failed` にする（成功へ倒さない）。
        let com = ComInit::new("media_cast");
        let mut res: Option<Res> = None;
        // 接続ごとの連番（ワーカー 1 本なので競合しない）。`release` を挟んでも戻さない。
        let mut conn_seq: u64 = 0;
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Prepare(gen, wav) => {
                    if with_shared(|s| s.check(gen)).is_err() {
                        continue;
                    }
                    if let Some(old) = res.take() {
                        teardown(old);
                    }
                    if com.is_err() {
                        fail(gen, CastFailure::Internal);
                        continue;
                    }
                    match create_player(gen, &wav, &tx) {
                        Ok(r) => res = Some(r),
                        Err(e) => fail(
                            gen,
                            if e.code().0 == CLASS_NOT_REG {
                                CastFailure::Unsupported
                            } else {
                                CastFailure::Os
                            },
                        ),
                    }
                }
                Cmd::Play(gen) => {
                    if let Some(r) = res.as_ref().filter(|r| r.gen == gen) {
                        if r.player.Play().is_err() {
                            fail(gen, CastFailure::Os);
                        }
                    }
                }
                Cmd::Stop(gen) => {
                    if let Some(r) = res.as_ref().filter(|r| r.gen == gen) {
                        let _ = r.player.Pause();
                        if let Ok(session) = r.player.PlaybackSession() {
                            let _ = session.SetPosition(TimeSpan { Duration: 0 });
                        }
                    }
                }
                Cmd::Selected(gen, device) => {
                    // 世代が違えば捨てる。一致すれば `Connecting` へ進めてから接続を始める。
                    conn_seq += 1;
                    let seq = conn_seq;
                    let name = device
                        .FriendlyName()
                        .map(|n| n.to_string())
                        .unwrap_or_default();
                    let current = with_shared(|s| {
                        let ok = s.check(gen).is_ok();
                        if ok {
                            s.on_event(gen, Ev::Selected(seq, name));
                        }
                        ok
                    });
                    if current {
                        // 資源が無い（準備失敗直後等）場合も 6 = Unknown として失敗を確定する。
                        let ok = match res.as_mut().filter(|r| r.gen == gen) {
                            Some(r) => connect(r, &device, seq, &tx).is_ok(),
                            None => false,
                        };
                        if !ok {
                            with_shared(|s| s.on_event(gen, Ev::StartResult(seq, 6)));
                        }
                    }
                }
                Cmd::Event(gen, ev) => with_shared(|s| s.on_event(gen, ev)),
                Cmd::Release => {
                    if let Some(old) = res.take() {
                        teardown(old);
                    }
                }
            }
        }
        if let Some(old) = res.take() {
            teardown(old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> (Shared, u64) {
        let mut s = Shared::new();
        let g = s.begin_prepare(10).unwrap();
        s.on_event(g, Ev::Opened);
        (s, g)
    }

    #[test]
    fn invalid_wav_is_rejected_and_invalidates_old_gen() {
        let (mut s, g) = ready();
        assert!(matches!(
            s.begin_prepare(0),
            Err(DeviceError::InvalidInput("media_cast wav"))
        ));
        assert!(s.check(g).is_err());
        assert!(s.begin_prepare(MAX_WAV_BYTES + 1).is_err());
        assert!(s.begin_prepare(MAX_WAV_BYTES).is_ok());
    }

    #[test]
    fn stale_generation_is_rejected_and_events_dropped() {
        let (mut s, g) = ready();
        let g2 = s.begin_prepare(10).unwrap();
        assert!(s.on_play(g).is_err() && s.on_stop(g).is_err() && s.on_picker_shown(g).is_err());
        s.on_event(g, Ev::Opened);
        s.on_event(g, Ev::Playing);
        s.on_event(g, Ev::ConnState(1, 1));
        assert_eq!(s.snapshot().playback, CastPlayback::Idle);
        assert_eq!(s.snapshot().generation, g2);
    }

    #[test]
    fn playing_only_after_os_notice_with_play_intent() {
        let (mut s, g) = ready();
        s.on_event(g, Ev::Playing);
        assert_eq!(s.playback, CastPlayback::Ready);
        s.on_play(g).unwrap();
        assert_eq!(s.playback, CastPlayback::Ready);
        // Playing 通知前の stop も OS へ流す（キュー内の Play が鳴らし続けないように）。
        assert!(s.on_stop(g).unwrap());
        s.on_event(g, Ev::Playing);
        assert_eq!(s.playback, CastPlayback::Ready);
        s.on_play(g).unwrap();
        s.on_event(g, Ev::Playing);
        assert_eq!(s.playback, CastPlayback::Playing);
        assert!(s.on_stop(g).unwrap());
        s.on_event(g, Ev::Paused);
        assert_eq!(s.playback, CastPlayback::Stopped);
        assert!(!s.on_stop(g).unwrap());
    }

    #[test]
    fn failed_is_not_overwritten_by_playing() {
        let (mut s, g) = ready();
        s.on_play(g).unwrap();
        s.on_event(g, Ev::MediaFailed(CastFailure::SourceNotSupported));
        s.on_event(g, Ev::Playing);
        assert_eq!(
            s.playback,
            CastPlayback::Failed(CastFailure::SourceNotSupported)
        );
        assert!(s.on_play(g).is_err());
    }

    #[test]
    fn dismissed_is_cancel_not_failure() {
        let (mut s, g) = ready();
        s.on_picker_shown(g).unwrap();
        assert_eq!(s.route, RouteState::Choosing);
        s.on_event(g, Ev::Dismissed);
        assert_eq!(s.route, RouteState::Local);
        assert_eq!(s.playback, CastPlayback::Ready);
    }

    #[test]
    fn picker_needs_prepared_state() {
        let mut s = Shared::new();
        let g = s.begin_prepare(10).unwrap();
        assert!(matches!(
            s.on_picker_shown(g),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn dismissed_order_does_not_undo_connecting() {
        let (mut s, g) = ready();
        s.on_picker_shown(g).unwrap();
        s.on_event(g, Ev::Selected(1, "TV".into()));
        s.on_event(g, Ev::Dismissed);
        assert_eq!(s.route, RouteState::Connecting);
        // 逆順（Dismissed -> Selected）でも Connecting に進む。
        let (mut s, g) = ready();
        s.on_picker_shown(g).unwrap();
        s.on_event(g, Ev::Dismissed);
        s.on_event(g, Ev::Selected(1, "TV".into()));
        assert_eq!(s.route, RouteState::Connecting);
    }

    #[test]
    fn connected_only_after_os_state_and_name_is_cleaned() {
        let (mut s, g) = ready();
        s.on_event(g, Ev::ConnState(1, 1));
        assert_eq!(s.route, RouteState::Local);
        s.on_event(g, Ev::Selected(1, " Living\u{0}Room\n".into()));
        assert_eq!(s.route, RouteState::Connecting);
        s.on_event(g, Ev::ConnState(1, 2));
        assert_eq!(
            s.route,
            RouteState::Connected {
                name: Some("LivingRoom".into())
            }
        );
        let (mut s, g) = ready();
        s.on_event(g, Ev::Selected(1, "\u{1}".into()));
        s.on_event(g, Ev::ConnState(1, 1));
        assert_eq!(s.route, RouteState::Connected { name: None });
    }

    #[test]
    fn start_status_mapping() {
        assert_eq!(failure_from_start_status(0), None);
        assert_eq!(failure_from_start_status(3), Some(CastFailure::Blocked));
        assert_eq!(
            failure_from_start_status(5),
            Some(CastFailure::SourceNotSupported)
        );
        for c in [1, 2, 4, 6, 99, -1] {
            assert_eq!(failure_from_start_status(c), Some(CastFailure::Os));
        }
        assert_eq!(failure_from_media_error(4), CastFailure::SourceNotSupported);
        assert_eq!(failure_from_media_error(2), CastFailure::Os);
    }

    #[test]
    fn start_failure_while_connecting_fails_once() {
        let (mut s, g) = ready();
        s.on_event(g, Ev::Selected(1, "TV".into()));
        s.on_event(g, Ev::StartResult(1, 3));
        assert_eq!(s.playback, CastPlayback::Failed(CastFailure::Blocked));
        assert_eq!(s.route, RouteState::Local);
        // 成功コードは状態を変えない。
        let (mut s, g) = ready();
        s.on_event(g, Ev::Selected(1, "TV".into()));
        s.on_event(g, Ev::StartResult(1, 0));
        assert_eq!(s.route, RouteState::Connecting);
    }

    #[test]
    fn error_after_connected_is_disconnect() {
        let (mut s, g) = ready();
        s.on_event(g, Ev::Selected(1, "TV".into()));
        s.on_event(g, Ev::ConnState(1, 1));
        s.on_event(g, Ev::ConnError(1, 2));
        assert_eq!(s.route, RouteState::Disconnected);
        assert_eq!(s.playback, CastPlayback::Ready);
        let (mut s, g) = ready();
        s.on_event(g, Ev::Selected(1, "TV".into()));
        s.on_event(g, Ev::ConnState(1, 1));
        s.on_event(g, Ev::ConnState(1, 0));
        assert_eq!(s.route, RouteState::Disconnected);
    }

    #[test]
    fn reset_drops_late_events() {
        let (mut s, g) = ready();
        s.reset();
        s.on_event(g, Ev::Playing);
        s.on_event(g, Ev::Selected(1, "TV".into()));
        s.on_event(g, Ev::ConnState(1, 1));
        let snap = s.snapshot();
        assert_eq!(snap.playback, CastPlayback::Idle);
        assert_eq!(snap.route, RouteState::Local);
        assert_eq!(snap.picker, PickerKind::App);
    }

    #[test]
    fn dismissed_keeps_route_when_already_connecting_or_connected() {
        let (mut s, g) = ready();
        s.on_event(g, Ev::Selected(1, "TV".into()));
        s.on_picker_shown(g).unwrap();
        assert_eq!(s.route, RouteState::Connecting);
        s.on_event(g, Ev::Dismissed);
        assert_eq!(s.route, RouteState::Connecting);
        s.on_event(g, Ev::ConnState(1, 1));
        s.on_picker_shown(g).unwrap();
        s.on_event(g, Ev::Dismissed);
        assert_eq!(
            s.route,
            RouteState::Connected {
                name: Some("TV".into())
            }
        );
        // 以後の ConnState も無視されない。
        s.on_event(g, Ev::ConnState(1, 0));
        assert_eq!(s.route, RouteState::Disconnected);
        s.on_picker_shown(g).unwrap();
        s.on_event(g, Ev::Dismissed);
        assert_eq!(s.route, RouteState::Disconnected);
    }

    #[test]
    fn stale_start_result_does_not_fail_new_connection() {
        let (mut s, g) = ready();
        s.on_event(g, Ev::Selected(1, "A".into()));
        s.on_event(g, Ev::Selected(2, "B".into()));
        // 旧接続（連番 1）の完了通知は捨てる。
        s.on_event(g, Ev::StartResult(1, 6));
        assert_eq!(s.route, RouteState::Connecting);
        assert_eq!(s.playback, CastPlayback::Ready);
        // 現在の接続の失敗は確定する。
        s.on_event(g, Ev::StartResult(2, 6));
        assert_eq!(s.route, RouteState::Local);
        assert_eq!(s.playback, CastPlayback::Failed(CastFailure::Os));
    }

    #[test]
    fn stale_conn_notices_are_dropped_after_switch() {
        let (mut s, g) = ready();
        s.on_event(g, Ev::Selected(1, "A".into()));
        s.on_event(g, Ev::Selected(2, "B".into()));
        s.on_event(g, Ev::ConnState(1, 1));
        assert_eq!(s.route, RouteState::Connecting);
        s.on_event(g, Ev::ConnError(1, 2));
        assert_eq!(s.route, RouteState::Connecting);
        assert_eq!(s.playback, CastPlayback::Ready);
        s.on_event(g, Ev::ConnState(2, 1));
        assert_eq!(
            s.route,
            RouteState::Connected {
                name: Some("B".into())
            }
        );
    }

    #[test]
    fn stop_before_playing_notice_still_reaches_stopped() {
        let (mut s, g) = ready();
        s.on_play(g).unwrap();
        assert!(s.on_stop(g).unwrap());
        s.on_event(g, Ev::Playing);
        s.on_event(g, Ev::Paused);
        assert_eq!(s.playback, CastPlayback::Stopped);
    }

    #[test]
    fn stale_selected_is_dropped() {
        let (mut s, g) = ready();
        s.on_event(g, Ev::Selected(2, "B".into()));
        s.on_event(g, Ev::Selected(1, "A".into()));
        assert_eq!(s.name.as_deref(), Some("B"));
        // reset 後は連番も戻り、旧連番の完了は新しい接続に効かない。
        s.reset();
        assert_eq!(s.conn_seq, 0);
    }
}
