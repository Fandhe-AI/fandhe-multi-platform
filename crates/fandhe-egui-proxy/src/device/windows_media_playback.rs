//! Windows 向け音声再生実装（Issue #268、親 #86）。`device/media_playback.rs::platform_playback` の
//! Windows 経路から呼ばれ、契約（SSOT）は `fandhe_device::media_playback`。
//! Media Foundation Media Engine（`IMFMediaEngine`、音声のみモード）で `demo_wav()` の WAV を
//! メモリから再生する。範囲は再生・一時停止・停止・位置・長さのみ（動画・シーク・ネットワークは範囲外）。
//!
//! # 方式: 専用ワーカー 1 本 + `mpsc` 直列キュー + 世代照合（契約表 2・11）
//!
//! 画面側（UI スレッド）の `load` / `play` / `pause` / `stop` は [`Shared`]（1 本の `Mutex`）を同期で
//! 更新してから `Cmd` を送り、待たずに戻る。MF の呼び出しはすべてワーカー（MTA）で行う
//! （UI スレッドは winit の STA で MTA 初期化できないため）。Media Engine の通知
//! （`IMFMediaEngineNotify`）は世代付きで `Cmd::Event` としてキューへ戻すだけで、通知コールバック内で
//! ロックもエンジン呼び出しもしない。判定と更新は同じロックの内側で行い、古い世代の通知は捨てる。
//! `Playing` は `PLAYING` イベントでだけ立て（嘘の成功の禁止、契約表 6）、`ENDED` と `ERROR` は別経路にする。
//! 位置・長さは OS の値をワーカーが読む（壁時計で数えない、契約表 3）。
//!
//! # mfplat.dll は動的に読み込む
//!
//! windows-rs の `MFStartup` 等の自由関数は import table に `mfplat.dll` を載せる。Media Feature Pack の
//! 無い Windows N エディションでは exe が起動できなくなり、本機能以外も巻き込む。そのため
//! `MFStartup` / `MFCreateAttributes` / `MFCreateMFByteStreamOnStream` は `LoadLibraryExW`
//! （`LOAD_LIBRARY_SEARCH_SYSTEM32` のみ。DLL planting 対策）+ `GetProcAddress` で解決し、
//! 失敗は `Failed(Unsupported)` にする。インターフェースのメソッドは vtable 呼び出しで import を生まない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の操作順、失敗を成功値へ畳まないこと、失敗経路を含むエンジンの `Shutdown()`、
//! HRESULT・MF のエラーコードをログ・画面へ出さないこと（A09）。窓が背面でも再生は続ける。
//! 守らない範囲: 位置表示のポーリング間隔ぶんの遅れ、他アプリの音声との混ざり、複数プロセス間。
//!
//! 既知の天井: `MFShutdown` は呼ばずプロセス終了に任せる。audio-only の Media Engine が
//! HWND / DXGI manager 無しで作れる前提と実機の挙動は未確認（#266）。

use std::sync::Arc;

#[cfg(not(target_os = "windows"))]
use fandhe_device::media_playback::UnsupportedMediaPlayback;
use fandhe_device::media_playback::{
    secs_to_ms, MediaPlayback, PlaybackFailure, PlaybackPhase, PlaybackSnapshot,
};
use fandhe_device::DeviceError;

/// `load` に渡せる WAV の上限（Apple 実装と同じ。異常に大きい入力を OS に渡さない）。
#[cfg(target_os = "windows")]
const MAX_WAV_BYTES: usize = 16 * 1024 * 1024;

/// 画面（ドライバ）が最後に求めた操作。遅れて届く通知を捨てる判断に使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intent {
    Play,
    Pause,
    Stop,
}

/// Media Engine の通知を、世代照合前の形で表したもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ev {
    /// `CANPLAY` / `LOADEDDATA`。
    CanPlay,
    Playing,
    Ended,
    /// `ERROR`（`MF_MEDIA_ENGINE_ERR` のコード）。
    Error(u32),
}

/// `MF_MEDIA_ENGINE_ERR` を契約の失敗種別へ写す（写像はここ 1 か所、契約表 5）。
/// 3 = DECODE、4 = SRC_NOT_SUPPORTED。生コードは保持しない。
fn failure_from_err(code: u32) -> PlaybackFailure {
    match code {
        3 | 4 => PlaybackFailure::Decode,
        _ => PlaybackFailure::Internal,
    }
}

/// 世代・状態・意図・位置を 1 本のロックで守る純ロジック（host でテストする）。
struct Shared {
    next_gen: u64,
    current: Option<u64>,
    phase: PlaybackPhase,
    intent: Intent,
    position_ms: Option<u64>,
    duration_ms: Option<u64>,
}

impl Shared {
    const fn new() -> Self {
        Self {
            next_gen: 0,
            current: None,
            phase: PlaybackPhase::Idle,
            intent: Intent::Stop,
            position_ms: None,
            duration_ms: None,
        }
    }

    /// 新しい読み込みを始める。旧世代は無効になる。`next_gen` は戻さない（ABA を防ぐ）。
    fn begin_load(&mut self) -> u64 {
        self.next_gen += 1;
        self.current = Some(self.next_gen);
        self.phase = PlaybackPhase::Loading;
        self.intent = Intent::Stop;
        self.position_ms = None;
        self.duration_ms = None;
        self.next_gen
    }

    fn check(&self, gen: u64) -> Result<(), DeviceError> {
        if self.current == Some(gen) {
            Ok(())
        } else {
            Err(DeviceError::InvalidInput("media_playback generation"))
        }
    }

    /// 意図だけ記録する。`Playing` は `PLAYING` 通知で立てる。
    fn on_play(&mut self, gen: u64) -> Result<(), DeviceError> {
        self.check(gen)?;
        self.intent = Intent::Play;
        Ok(())
    }

    fn on_pause(&mut self, gen: u64) -> Result<(), DeviceError> {
        self.check(gen)?;
        self.intent = Intent::Pause;
        if self.phase == PlaybackPhase::Playing {
            self.phase = PlaybackPhase::Paused;
        }
        Ok(())
    }

    fn on_stop(&mut self, gen: u64) -> Result<(), DeviceError> {
        self.check(gen)?;
        self.intent = Intent::Stop;
        if !matches!(
            self.phase,
            PlaybackPhase::Loading | PlaybackPhase::Failed(_)
        ) {
            self.phase = PlaybackPhase::Ready;
            self.position_ms = Some(0);
        }
        Ok(())
    }

    /// 世代の違う通知は捨てる（契約表 2）。
    fn on_event(&mut self, gen: u64, ev: Ev) {
        if self.check(gen).is_err() {
            return;
        }
        match ev {
            Ev::CanPlay => {
                if self.phase == PlaybackPhase::Loading {
                    self.phase = PlaybackPhase::Ready;
                }
            }
            // 期限切れ・一時停止の後に遅れて届いた PLAYING は捨てる（契約表 3）。
            Ev::Playing => {
                if self.intent == Intent::Play {
                    self.phase = PlaybackPhase::Playing;
                }
            }
            Ev::Ended => {
                if self.intent == Intent::Play && self.phase == PlaybackPhase::Playing {
                    self.phase = PlaybackPhase::Ended;
                    self.intent = Intent::Stop;
                }
            }
            Ev::Error(code) => self.phase = PlaybackPhase::Failed(failure_from_err(code)),
        }
    }

    /// OS の秒値を写す。NaN・無限大・負値は `None`（範囲検証は丸めより前）。
    fn on_times(&mut self, gen: u64, pos_secs: f64, dur_secs: f64) {
        if self.check(gen).is_ok() {
            self.position_ms = secs_to_ms(pos_secs);
            self.duration_ms = secs_to_ms(dur_secs);
        }
    }

    fn fail(&mut self, gen: u64, f: PlaybackFailure) {
        if self.check(gen).is_ok() {
            self.phase = PlaybackPhase::Failed(f);
        }
    }

    /// 以後の古い世代の操作を `Err` にする（`shutdown` 用）。
    fn reset(&mut self) {
        self.current = None;
        self.phase = PlaybackPhase::Idle;
        self.intent = Intent::Stop;
        self.position_ms = None;
        self.duration_ms = None;
    }

    fn snapshot(&self) -> PlaybackSnapshot {
        PlaybackSnapshot {
            generation: self.current.unwrap_or(0),
            phase: self.phase,
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
        }
    }
}

/// 現在の実装。
pub(crate) fn implementation() -> Arc<dyn MediaPlayback> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(win::WindowsMediaPlayback)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Arc::new(UnsupportedMediaPlayback)
    }
}

#[cfg(target_os = "windows")]
mod win {
    use std::ffi::c_void;
    use std::sync::{mpsc, Mutex, OnceLock};

    use windows::core::{implement, s, w, Interface, BSTR, HRESULT};
    use windows::Win32::Media::MediaFoundation::{
        CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFByteStream, IMFMediaEngineClassFactory,
        IMFMediaEngineEx, IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, MFSTARTUP_FULL,
        MF_MEDIA_ENGINE_AUDIOONLY, MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_EVENT_CANPLAY,
        MF_MEDIA_ENGINE_EVENT_ENDED, MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDDATA,
        MF_MEDIA_ENGINE_EVENT_PLAYING, MF_VERSION,
    };
    use windows::Win32::System::Com::{CoCreateInstance, IStream, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    use windows::Win32::UI::Shell::SHCreateMemStream;

    use fandhe_device::media_playback::POLL_INTERVAL;

    use super::*;
    use crate::device::windows_notification::ComInit;

    /// `REGDB_E_CLASSNOTREG`（Media Engine が登録されていない環境）。
    const CLASS_NOT_REG: i32 = 0x8004_0154_u32 as i32;

    static SHARED: Mutex<Shared> = Mutex::new(Shared::new());

    /// ロックの poison は無視して進める。
    fn with_shared<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
        f(&mut SHARED.lock().unwrap_or_else(|e| e.into_inner()))
    }

    enum Cmd {
        Load(u64, Arc<[u8]>),
        Play(u64),
        Pause(u64),
        Stop(u64),
        Event(u64, Ev),
        Shutdown,
    }

    /// `None` はワーカーを立てられなかったことの記録（以後の `load` は失敗）。
    static QUEUE: OnceLock<Option<mpsc::Sender<Cmd>>> = OnceLock::new();

    fn queue() -> Option<&'static mpsc::Sender<Cmd>> {
        QUEUE
            .get_or_init(|| {
                let (tx, rx) = mpsc::channel();
                let tx2 = tx.clone();
                std::thread::Builder::new()
                    .name("fandhe-media-playback".into())
                    .spawn(move || worker(rx, tx2))
                    .ok()
                    .map(|_| tx)
            })
            .as_ref()
    }

    fn send(cmd: Cmd) -> Result<(), DeviceError> {
        match queue() {
            Some(q) if q.send(cmd).is_ok() => Ok(()),
            _ => Err(DeviceError::Internal("media_playback")),
        }
    }

    pub(super) struct WindowsMediaPlayback;

    impl MediaPlayback for WindowsMediaPlayback {
        fn load(&self, wav: Arc<[u8]>) -> Result<u64, DeviceError> {
            if wav.is_empty() || wav.len() > MAX_WAV_BYTES {
                // 旧世代も無効化する（Apple 実装と同じ）。
                self.shutdown();
                return Err(DeviceError::InvalidInput("media_playback wav"));
            }
            let gen = with_shared(|s| s.begin_load());
            if let Err(e) = send(Cmd::Load(gen, wav)) {
                with_shared(|s| s.fail(gen, PlaybackFailure::Internal));
                return Err(e);
            }
            Ok(gen)
        }

        fn play(&self, generation: u64) -> Result<(), DeviceError> {
            with_shared(|s| s.on_play(generation))?;
            send(Cmd::Play(generation))
        }

        fn pause(&self, generation: u64) -> Result<(), DeviceError> {
            with_shared(|s| s.on_pause(generation))?;
            send(Cmd::Pause(generation))
        }

        fn stop(&self, generation: u64) -> Result<(), DeviceError> {
            with_shared(|s| s.on_stop(generation))?;
            send(Cmd::Stop(generation))
        }

        fn snapshot(&self) -> PlaybackSnapshot {
            with_shared(|s| s.snapshot())
        }

        fn shutdown(&self) {
            with_shared(|s| s.reset());
            // ワーカー未起動なら片付ける物が無い。送信失敗も同様に無視する。
            if let Some(q) = QUEUE.get().and_then(|q| q.as_ref()) {
                let _ = q.send(Cmd::Shutdown);
            }
        }
    }

    /// `#[implement]` が生成する COM vtable コードは `unsafe` を含むため、ここだけ許可する。
    mod notify {
        #![allow(unsafe_code)]
        use super::*;

        /// Media Engine の通知をキューへ戻すだけの受け口。ロックもエンジン呼び出しもしない。
        #[implement(IMFMediaEngineNotify)]
        pub(super) struct Notify {
            pub(super) gen: u64,
            pub(super) tx: mpsc::Sender<Cmd>,
        }

        impl IMFMediaEngineNotify_Impl for Notify_Impl {
            fn EventNotify(
                &self,
                event: u32,
                param1: usize,
                _param2: u32,
            ) -> windows::core::Result<()> {
                let ev = if event == MF_MEDIA_ENGINE_EVENT_CANPLAY.0 as u32
                    || event == MF_MEDIA_ENGINE_EVENT_LOADEDDATA.0 as u32
                {
                    Some(Ev::CanPlay)
                } else if event == MF_MEDIA_ENGINE_EVENT_PLAYING.0 as u32 {
                    Some(Ev::Playing)
                } else if event == MF_MEDIA_ENGINE_EVENT_ENDED.0 as u32 {
                    Some(Ev::Ended)
                } else if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
                    Some(Ev::Error(param1 as u32))
                } else {
                    None
                };
                if let Some(ev) = ev {
                    let _ = self.tx.send(Cmd::Event(self.gen, ev));
                }
                Ok(())
            }
        }
    }
    use notify::Notify;

    /// 動的に解決した mfplat.dll の 3 関数（シグネチャは MS Learn の mfapi.h と一致させる）。
    struct Mf {
        startup: unsafe extern "system" fn(u32, u32) -> HRESULT,
        create_attributes: unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT,
        bytestream_on_stream: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    }

    impl Mf {
        /// 読み込み・関数解決・`MFStartup` のどれかが失敗したら `None`（非対応）。
        /// `HMODULE` はプロセス寿命で保持する（`FreeLibrary` しない）。
        // 変換先の型は Mf のフィールド宣言で固定されている（注釈を足しても重複するだけ）。
        #[allow(unsafe_code, clippy::missing_transmute_annotations)]
        fn init() -> Option<Self> {
            // SAFETY: 固定の DLL 名を System32 からのみ読み込む（DLL planting 対策）。
            let module =
                unsafe { LoadLibraryExW(w!("mfplat.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
                    .ok()?;
            // SAFETY: `module` は上で得た有効なハンドル。名前は NUL 終端のリテラル。
            // 戻り値の関数ポインタは MS Learn の宣言（MFStartup(ULONG, DWORD) /
            // MFCreateAttributes(IMFAttributes**, UINT32) /
            // MFCreateMFByteStreamOnStream(IStream*, IMFByteStream**)、いずれも STDAPI）と
            // 同じ ABI・引数型の fn 型へ変換する。
            let mf = unsafe {
                use std::mem::transmute;
                Self {
                    startup: transmute(GetProcAddress(module, s!("MFStartup"))?),
                    create_attributes: transmute(GetProcAddress(module, s!("MFCreateAttributes"))?),
                    bytestream_on_stream: transmute(GetProcAddress(
                        module,
                        s!("MFCreateMFByteStreamOnStream"),
                    )?),
                }
            };
            // SAFETY: 上で解決した MFStartup。プロセスで 1 回だけ呼ぶ（`MFShutdown` は呼ばない）。
            unsafe { (mf.startup)(MF_VERSION, MFSTARTUP_FULL) }
                .is_ok()
                .then_some(mf)
        }

        #[allow(unsafe_code)]
        fn attributes(&self) -> Option<IMFAttributes> {
            let mut raw = std::ptr::null_mut();
            // SAFETY: 出力先は有効なポインタ変数。成功時は AddRef 済みの IMFAttributes を所有する。
            unsafe {
                (self.create_attributes)(&mut raw, 1)
                    .is_ok()
                    .then(|| IMFAttributes::from_raw(raw))
            }
        }

        #[allow(unsafe_code)]
        fn byte_stream(&self, stream: &IStream) -> Option<IMFByteStream> {
            let mut raw = std::ptr::null_mut();
            // SAFETY: `stream` は生存中の IStream。成功時は AddRef 済みの IMFByteStream を所有する。
            unsafe {
                (self.bytestream_on_stream)(stream.as_raw(), &mut raw)
                    .is_ok()
                    .then(|| IMFByteStream::from_raw(raw))
            }
        }
    }

    /// エンジンを作って WAV を渡す。失敗時は作りかけのエンジンを `Shutdown()` してから返す（契約表 9）。
    #[allow(unsafe_code)]
    fn create_engine(
        mf: &Mf,
        gen: u64,
        tx: &mpsc::Sender<Cmd>,
        wav: &[u8],
    ) -> Result<IMFMediaEngineEx, PlaybackFailure> {
        let internal = |_| PlaybackFailure::Internal;
        // SAFETY: COM 初期化済みのワーカースレッドから、有効な CLSID で生成する。
        let factory: IMFMediaEngineClassFactory = unsafe {
            CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
        }
        .map_err(|e| {
            if e.code().0 == CLASS_NOT_REG {
                PlaybackFailure::Unsupported
            } else {
                PlaybackFailure::Internal
            }
        })?;
        let attrs = mf.attributes().ok_or(PlaybackFailure::Internal)?;
        let notify: IMFMediaEngineNotify = Notify {
            gen,
            tx: tx.clone(),
        }
        .into();
        // SAFETY: `attrs` / `notify` は生存中の COM オブジェクト。キーは MF_MEDIA_ENGINE_CALLBACK。
        let engine: IMFMediaEngineEx = unsafe {
            attrs
                .SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)
                .map_err(internal)?;
            factory
                .CreateInstance(MF_MEDIA_ENGINE_AUDIOONLY.0 as u32, &attrs)
                .map_err(internal)?
                .cast()
                .map_err(internal)?
        };
        // SAFETY: `SHCreateMemStream` は `wav` を同期でコピーするため Rust 側バッファを保持しない。
        let source = unsafe { SHCreateMemStream(Some(wav)) }
            .and_then(|s| mf.byte_stream(&s))
            .ok_or(PlaybackFailure::Internal)
            .and_then(|bs| {
                // SAFETY: `engine` / `bs` は生存中。URL は形式判定用の固定名。
                unsafe { engine.SetSourceFromByteStream(&bs, &BSTR::from("fandhe.wav")) }
                    .map_err(|_| PlaybackFailure::Decode)
            });
        if let Err(f) = source {
            shutdown_engine(&engine);
            return Err(f);
        }
        Ok(engine)
    }

    #[allow(unsafe_code)]
    fn shutdown_engine(engine: &IMFMediaEngineEx) {
        // SAFETY: 生存中のエンジンに対する解放要求。失敗しても以後 drop で解放される。
        let _ = unsafe { engine.Shutdown() };
    }

    /// `Play` / `Pause` / `Stop` をエンジンへ流す。
    #[allow(unsafe_code)]
    fn drive(engine: &IMFMediaEngineEx, cmd: &Cmd) -> windows::core::Result<()> {
        // SAFETY: 生存中のエンジンへの単純な操作呼び出し（ワーカーからのみ）。
        unsafe {
            match cmd {
                Cmd::Play(_) => engine.Play(),
                Cmd::Pause(_) => engine.Pause(),
                _ => {
                    engine.Pause()?;
                    engine.SetCurrentTime(0.0)
                }
            }
        }
    }

    #[allow(unsafe_code)]
    fn times(engine: &IMFMediaEngineEx) -> (f64, f64) {
        // SAFETY: 生存中のエンジンの読み取り。NaN は `secs_to_ms` が `None` にする。
        unsafe { (engine.GetCurrentTime(), engine.GetDuration()) }
    }

    fn worker(rx: mpsc::Receiver<Cmd>, tx: mpsc::Sender<Cmd>) {
        // 失敗しても worker は続け、以後の `Load` を `Failed` にする（成功へ倒さない）。
        let com = ComInit::new("media_playback");
        // 外側 None = 未試行。MFStartup は最初の Load で 1 回だけ。
        let mut mf: Option<Option<Mf>> = None;
        let mut engine: Option<(u64, IMFMediaEngineEx)> = None;
        loop {
            let playing = engine.is_some() && with_shared(|s| s.intent == Intent::Play);
            let cmd = if playing {
                match rx.recv_timeout(POLL_INTERVAL) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => break,
                }
            };
            match cmd {
                None => {}
                Some(Cmd::Load(gen, wav)) => {
                    if with_shared(|s| s.check(gen)).is_err() {
                        continue;
                    }
                    if let Some((_, old)) = engine.take() {
                        shutdown_engine(&old);
                    }
                    let result = if com.is_err() {
                        Err(PlaybackFailure::Internal)
                    } else {
                        match mf.get_or_insert_with(Mf::init) {
                            Some(mf) => create_engine(mf, gen, &tx, &wav),
                            None => Err(PlaybackFailure::Unsupported),
                        }
                    };
                    match result {
                        Ok(e) => engine = Some((gen, e)),
                        Err(f) => with_shared(|s| s.fail(gen, f)),
                    }
                }
                Some(Cmd::Event(gen, ev)) => with_shared(|s| s.on_event(gen, ev)),
                Some(Cmd::Shutdown) => {
                    if let Some((_, old)) = engine.take() {
                        shutdown_engine(&old);
                    }
                }
                Some(ref c @ (Cmd::Play(g) | Cmd::Pause(g) | Cmd::Stop(g))) => {
                    if let Some((eg, e)) = engine.as_ref().filter(|(eg, _)| *eg == g) {
                        if drive(e, c).is_err() {
                            let eg = *eg;
                            with_shared(|s| s.fail(eg, PlaybackFailure::Internal));
                        }
                    }
                }
            }
            if let Some((g, e)) = engine.as_ref() {
                let (p, d) = times(e);
                with_shared(|s| s.on_times(*g, p, d));
            }
        }
        // ponytail: MFShutdown は呼ばない（プロセス終了に任せる）。天井: 長寿命プロセスで
        // 複数回の startup/shutdown 対が必要になったら参照カウントを持つ。
        if let Some((_, e)) = engine.take() {
            shutdown_engine(&e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> (Shared, u64) {
        let mut s = Shared::new();
        let g = s.begin_load();
        (s, g)
    }

    #[test]
    fn stale_generation_is_rejected_after_reload_and_reset() {
        let (mut s, g) = loaded();
        let g2 = s.begin_load();
        assert_ne!(g, g2);
        assert!(s.on_play(g).is_err() && s.on_pause(g).is_err() && s.on_stop(g).is_err());
        assert!(s.on_play(g2).is_ok());
        s.reset();
        assert!(s.on_play(g2).is_err());
        assert_eq!(s.snapshot().phase, PlaybackPhase::Idle);
    }

    #[test]
    fn late_playing_is_dropped_unless_intent_is_play() {
        let (mut s, g) = loaded();
        s.on_event(g, Ev::Playing);
        assert_eq!(s.phase, PlaybackPhase::Loading);
        s.on_play(g).unwrap();
        s.on_event(g, Ev::Playing);
        assert_eq!(s.phase, PlaybackPhase::Playing);
        s.on_pause(g).unwrap();
        s.on_event(g, Ev::Playing);
        assert_eq!(s.phase, PlaybackPhase::Paused);
    }

    #[test]
    fn ended_and_error_are_distinct_and_ended_needs_playing() {
        let (mut s, g) = loaded();
        s.on_event(g, Ev::Ended);
        assert_eq!(s.phase, PlaybackPhase::Loading);
        s.on_play(g).unwrap();
        s.on_event(g, Ev::Playing);
        s.on_event(g, Ev::Ended);
        assert_eq!(s.phase, PlaybackPhase::Ended);
        let (mut s, g) = loaded();
        s.on_event(g, Ev::Error(3));
        assert_eq!(s.phase, PlaybackPhase::Failed(PlaybackFailure::Decode));
    }

    #[test]
    fn events_of_other_generation_are_dropped() {
        let (mut s, g) = loaded();
        s.on_event(g + 1, Ev::CanPlay);
        s.on_event(g + 1, Ev::Error(3));
        assert_eq!(s.phase, PlaybackPhase::Loading);
        s.on_event(g, Ev::CanPlay);
        assert_eq!(s.phase, PlaybackPhase::Ready);
    }

    #[test]
    fn failure_mapping() {
        for c in [3, 4] {
            assert_eq!(failure_from_err(c), PlaybackFailure::Decode);
        }
        for c in [0, 1, 2, 5, 99] {
            assert_eq!(failure_from_err(c), PlaybackFailure::Internal);
        }
    }

    #[test]
    fn times_reject_invalid_and_stop_rewinds() {
        let (mut s, g) = loaded();
        s.on_times(g, f64::NAN, f64::INFINITY);
        assert_eq!((s.position_ms, s.duration_ms), (None, None));
        s.on_times(g, -1.0, 5.0);
        assert_eq!((s.position_ms, s.duration_ms), (None, Some(5000)));
        s.on_event(g, Ev::CanPlay);
        s.on_times(g, 2.5, 5.0);
        s.on_stop(g).unwrap();
        assert_eq!(s.position_ms, Some(0));
        assert_eq!(s.phase, PlaybackPhase::Ready);
    }
}
