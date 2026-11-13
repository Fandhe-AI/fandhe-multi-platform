//! Windows 向け動画の撮影実装（Issue #1150）。`device/video_capture.rs` の `platform()`
//! （Windows 経路）から呼ばれ、WinRT `MediaCapture` で動画を MP4 に録り、撮れた動画の長さ・寸法・
//! 向き・ファイルサイズを `crate::contract::VideoClip::from_os` で検査して返す。
//! 契約（三値・数値表・守る範囲）の SSOT は `crate::contract`。OS の撮影 UI が無いため、
//! 撮影パネル（開始・停止・キャンセル）は core 画面（`fandhe_core::screens::video_capture_show`）が描き、
//! 本モジュールは状態を [`crate::driver::set_panel`] で書き、命令を [`panel_command`] で受ける。
//!
//! # 流れ（専用ワーカーの待たない状態機械）
//!
//! 1. 要求ごとに新しい一時置き場を作り（[`TempClip`]）、`StorageFile::GetFileFromPathAsync`
//! 2. `InitializeWithSettingsAsync`（`StreamingCaptureMode::Video` のみ。音声は録らない）。成功したら
//!    `try_begin_present` を記録してパネルを `Ready` にする
//! 3. 「録画を始める」で `StartRecordToStorageFileAsync`（MP4・音声なし）。始まったらパネルを `Recording` にし、
//!    期限（[`crate::contract::MAX_DURATION_SECS`]）を単調時計（`Instant`）で測る
//! 4. 「止める」・期限切れ・「キャンセル」で `StopRecordAsync`。撮れたなら `GetVideoPropertiesAsync` で
//!    長さ・寸法・向きを読み、ファイルの大きさと合わせて検査する
//! 5. 後片付け（下記）を済ませてから `complete_from_os` を 1 回だけ呼ぶ
//!
//! 各段の完了は `SetCompleted` から [`Cmd`] としてキューへ積む。ワーカーが待つのは `recv`
//! （録画中だけ期限つきの `recv_timeout`）だけで、ワーカー上で `.get()` は使わない。
//!
//! # OS を正にする
//!
//! パネルの状態は、ワーカーが OS の完了を観測した後にだけ書く。ボタンは命令を立てるだけで、命令は
//! ワーカーが現在の段と照合して受け付けるか捨てる（`Start` は `Ready`、`Stop` は `Recording`、
//! `Cancel` は `Ready` / `Recording` のときだけ）。effect が同じ命令を複数回転送しても重複は捨てられる。
//!
//! # 並行性（契約表 2・11）
//!
//! 要求・パネル命令・各段の完了・OS の通知は直列キュー（[`Cmd`]）1 本に集約し、状態（[`Job`]）は
//! ワーカーのローカルにだけ置く。古い要求 id の完了・命令は照合して捨てる。同時に動かすのは 1 件で、
//! 処理中に来た別の要求は `Internal` にする（bridge が同時 1 件を保証するため通常は起きない）。
//!
//! # 解放（契約表 5・9）
//!
//! 全経路が [`Worker::finish`] を通る。保留中の非同期操作は `Cancel` して、完了（取り消しを含む）が
//! 届いてから、`RemoveFailed` → `StorageFile` の drop → `MediaCapture::Close` → 置き場の削除の順に解放する。
//! 解放のどれかが失敗したら、読めた結果（撮れた・取り消し）があっても `Internal` にする。消せなかった置き場は
//! ワーカー（プロセスと同じだけ生きる）の `leftovers` に記録し、次の要求の前に消し直して、消せなければ
//! その要求も `Internal` にする。
//!
//! 動画の中身・パス・HRESULT・OS のエラー文はログにも結果にも出さない（契約表 8）。
//! `unsafe` と `RoInitialize` は使わない。
//!
//! # 既知の天井
//!
//! `InitializeAsync` は公式には STA 指定で、本実装は MTA のワーカーで呼ぶ（camera-preview-stream
//! の Windows #1112 と同じ。実機で失敗したら UI スレッドでの初期化へ寄せる）。Windows 実機での撮影・
//! 取り消し・30 秒の自動停止・権限拒否・カメラなしの挙動は #819 で確認する。保留中の操作の `Cancel`
//! に対する完了が届かない場合、その要求は完了しない（期限を持たない守らない範囲）。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::contract::VideoCaptureFailure;

/// `E_ACCESSDENIED`（カメラへのアクセス拒否）。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
/// `MF_E_NO_CAPTURE_DEVICES_AVAILABLE`（撮影できるカメラが無い）。
const MF_E_NO_CAPTURE_DEVICES_AVAILABLE: i32 = 0xC00D_ABE0_u32 as i32;

/// 開始・録画の失敗 HRESULT を契約上の失敗へ写す。拒否は権限拒否、カメラ無しは非対応
/// （macOS の「カメラが無い」と同じ扱い）、それ以外は `Internal`。成功へは倒さない。
pub(crate) fn failure_from_hresult(code: i32) -> VideoCaptureFailure {
    match code {
        E_ACCESSDENIED => VideoCaptureFailure::PermissionDenied,
        MF_E_NO_CAPTURE_DEVICES_AVAILABLE => VideoCaptureFailure::Unsupported,
        _ => VideoCaptureFailure::Internal,
    }
}

/// `TimeSpan`（100 ns 単位）をミリ秒へ。0 以下は長さなし（0 を作らない）。
pub(crate) fn duration_ms_from_ticks(ticks: i64) -> Option<u64> {
    u64::try_from(ticks)
        .ok()
        .map(|t| t / 10_000)
        .filter(|&ms| ms > 0)
}

/// 置き場の名前の採番（プロセス内で一意にする）。
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 要求ごとに新しく作る一時置き場と、その中の動画ファイル。このプロセスが作って追跡している分だけを
/// 消す（共有の一時領域を走査しない。契約表 9）。
pub(crate) struct TempClip {
    dir: PathBuf,
    file: PathBuf,
}

impl TempClip {
    /// OS の一時領域の下に新しい置き場と空の動画ファイルを作る。
    pub(crate) fn create() -> io::Result<TempClip> {
        Self::create_in(&std::env::temp_dir())
    }

    /// `base` の下に置き場を作る。既にある名前（再利用・差し替えの疑い）は使わず別名にする。
    pub(crate) fn create_in(base: &Path) -> io::Result<TempClip> {
        for _ in 0..16 {
            let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = base.join(format!("fandhe-video-capture-{}-{n}", std::process::id()));
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
            let file = dir.join("clip.mp4");
            // 既存のファイル・リンクは `create_new` が失敗するので、書き込み先として使わない。
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
            {
                Ok(_) => return Ok(TempClip { dir, file }),
                Err(e) => {
                    let _ = fs::remove_dir_all(&dir);
                    return Err(e);
                }
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }

    /// 動画ファイルのパス。
    pub(crate) fn path(&self) -> &Path {
        &self.file
    }

    /// 動画ファイルの大きさ（バイト）。
    pub(crate) fn size(&self) -> io::Result<u64> {
        fs::metadata(&self.file).map(|m| m.len())
    }

    /// 置き場ごと消す。失敗したら消し直すための置き場のパスを返す。
    pub(crate) fn remove(self) -> Result<(), PathBuf> {
        match fs::remove_dir_all(&self.dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(self.dir),
        }
    }
}

/// 消せなかった置き場を消し直す。消せたものは外し、全部消せたら `true`。
pub(crate) fn retry_leftovers(leftovers: &mut Vec<PathBuf>) -> bool {
    leftovers.retain(|dir| match fs::remove_dir_all(dir) {
        Ok(()) => false,
        Err(e) => e.kind() != io::ErrorKind::NotFound,
    });
    leftovers.is_empty()
}

#[cfg(target_os = "windows")]
pub(crate) use imp::{implementation, panel_command};

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::{Arc, OnceLock};
    use std::time::{Duration, Instant};

    use windows::Media::Capture::{
        MediaCapture, MediaCaptureFailedEventHandler, MediaCaptureInitializationSettings,
        StreamingCaptureMode,
    };
    use windows::Media::MediaProperties::{
        AudioEncodingProperties, MediaEncodingProfile, VideoEncodingQuality,
    };
    use windows::Storage::FileProperties::VideoProperties;
    use windows::Storage::StorageFile;
    use windows::core::{AgileReference, HSTRING};
    use windows_future::IAsyncInfo;

    use crate::contract::{VideoCaptureFailure, VideoCaptureOutcome, VideoCapturer, VideoClip};
    use fandhe_core::screens::video_capture_show::{PanelCommand, PanelPhase};

    use super::{TempClip, duration_ms_from_ticks, failure_from_hresult, retry_leftovers};
    use crate::driver::{complete_from_os, set_panel, try_begin_present};
    use fandhe_platform_windows::winrt_async::{spawn, spawn_action};

    type Outcome = Result<VideoCaptureOutcome, VideoCaptureFailure>;

    /// ワーカーへの命令。要求・パネル命令・各段の完了・OS の通知はすべてここを順に通る。
    enum Cmd {
        Capture(u64, u32),
        Panel(u64, PanelCommand),
        /// `StorageFile` は別スレッドへ渡せないため、`AgileReference` で運ぶ。
        File(u64, windows::core::Result<AgileReference<StorageFile>>),
        Initialized(u64, windows::core::Result<()>),
        Started(u64, windows::core::Result<()>),
        Stopped(u64, windows::core::Result<()>),
        Props(u64, windows::core::Result<ClipProps>),
        CaptureFailed(u64),
    }

    impl Cmd {
        /// 非同期操作の完了を運ぶ命令の要求 id。
        fn completion_id(&self) -> Option<u64> {
            match self {
                Cmd::File(id, _)
                | Cmd::Initialized(id, _)
                | Cmd::Started(id, _)
                | Cmd::Stopped(id, _)
                | Cmd::Props(id, _) => Some(*id),
                _ => None,
            }
        }
    }

    /// `VideoProperties` から完了ハンドラ内で読み出した値（WinRT の型をスレッドをまたいで運ばない）。
    struct ClipProps {
        ticks: i64,
        width: u32,
        height: u32,
        rotation: i32,
    }

    fn read_props(p: VideoProperties) -> windows::core::Result<ClipProps> {
        Ok(ClipProps {
            ticks: p.Duration().map(|d| d.Duration).unwrap_or(0),
            width: p.Width()?,
            height: p.Height()?,
            rotation: p.Orientation()?.0,
        })
    }

    /// 1 要求が今どの段にいるか。
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Stage {
        /// ファイルの取得・初期化の途中（パネル未表示）。
        Opening,
        /// 準備完了。録画の開始待ち。
        Ready,
        /// `StartRecordToStorageFileAsync` の完了待ち。
        Starting,
        Recording,
        /// `StopRecordAsync` の完了待ち。
        Stopping,
        /// `GetVideoPropertiesAsync` の完了待ち。
        Reading,
        /// 結果は決まった。保留中の操作の完了を待って解放する。
        Closing,
    }

    /// 1 要求ぶんの WinRT 資源と状態。
    struct Job {
        id: u64,
        max: Duration,
        stage: Stage,
        clip: Option<TempClip>,
        file: Option<StorageFile>,
        capture: Option<MediaCapture>,
        failed_token: Option<i64>,
        /// 保留中の非同期操作。完了の [`Cmd`] が届くまで `None` にしない。
        op: Option<IAsyncInfo>,
        /// 録画中の期限（単調時計）。
        deadline: Option<Instant>,
        /// 取り消しのために止めている途中か。
        cancelling: bool,
        /// 解放後に返す結果。
        result: Option<Outcome>,
    }

    struct Worker {
        tx: Sender<Cmd>,
        job: Option<Job>,
        /// 消せなかった置き場。要求より長く生きる（ワーカーはプロセスと同じだけ生きる）。
        leftovers: Vec<std::path::PathBuf>,
    }

    static WORKER: OnceLock<Option<Sender<Cmd>>> = OnceLock::new();

    fn worker() -> Option<&'static Sender<Cmd>> {
        WORKER
            .get_or_init(|| {
                let (tx, rx) = mpsc::channel();
                let own = tx.clone();
                std::thread::Builder::new()
                    .name("video-capture".into())
                    .spawn(move || run_worker(rx, own))
                    .ok()
                    .map(|_| tx)
            })
            .as_ref()
    }

    struct WindowsVideoCapturer;

    impl VideoCapturer for WindowsVideoCapturer {
        fn capture(
            &self,
            request_id: u64,
            max_duration_secs: u32,
        ) -> Result<(), VideoCaptureFailure> {
            worker()
                .and_then(|tx| tx.send(Cmd::Capture(request_id, max_duration_secs)).ok())
                .ok_or(VideoCaptureFailure::Internal)
        }
    }

    /// `device/video_capture.rs` から呼ばれる、プロセスに 1 つの実装。
    pub(crate) fn implementation() -> Arc<dyn VideoCapturer> {
        Arc::new(WindowsVideoCapturer)
    }

    /// 撮影パネルのボタンの命令をワーカーへ積む（effect ドライバから）。
    pub(crate) fn panel_command(request_id: u64, command: PanelCommand) {
        if let Some(tx) = WORKER.get().and_then(|w| w.as_ref()) {
            let _ = tx.send(Cmd::Panel(request_id, command));
        }
    }

    fn run_worker(rx: Receiver<Cmd>, tx: Sender<Cmd>) {
        let mut w = Worker {
            tx,
            job: None,
            leftovers: Vec::new(),
        };
        loop {
            let deadline = w.job.as_ref().and_then(|j| j.deadline);
            let cmd = match deadline {
                Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                    Ok(c) => c,
                    Err(RecvTimeoutError::Timeout) => {
                        w.deadline_reached();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(c) => c,
                    Err(_) => break,
                },
            };
            w.step(cmd);
        }
    }

    impl Worker {
        fn cur(&mut self, id: u64) -> Option<&mut Job> {
            self.job.as_mut().filter(|j| j.id == id)
        }

        fn step(&mut self, cmd: Cmd) {
            // 解放待ちの要求に一致する完了は、中身を捨てて解放の続きとして扱う。
            if let Some(id) = cmd.completion_id()
                && self
                    .job
                    .as_ref()
                    .is_some_and(|j| j.id == id && j.stage == Stage::Closing)
            {
                if let Some(j) = self.job.as_mut() {
                    j.op = None;
                }
                self.release();
                return;
            }
            match cmd {
                Cmd::Capture(id, max) => self.begin(id, max),
                Cmd::Panel(id, c) => self.panel(id, c),
                Cmd::File(id, r) => self.after_file(id, r),
                Cmd::Initialized(id, r) => self.after_init(id, r),
                Cmd::Started(id, r) => self.after_started(id, r),
                Cmd::Stopped(id, r) => self.after_stopped(id, r),
                Cmd::Props(id, r) => self.after_props(id, r),
                Cmd::CaptureFailed(id) => self.capture_failed(id),
            }
        }

        /// 新しい要求を始める。置き場を作ってファイルを取得する。
        fn begin(&mut self, id: u64, max: u32) {
            if self.job.is_some() {
                // bridge が同時 1 件を保証する。来たら進行中の要求を壊さず、新しい要求だけ失敗にする。
                complete_from_os(id, Err(VideoCaptureFailure::Internal));
                return;
            }
            // 前の要求で消せなかった置き場を消し直す。消せなければ今回も失敗にする（契約表 9）。
            if !retry_leftovers(&mut self.leftovers) {
                complete_from_os(id, Err(VideoCaptureFailure::Internal));
                return;
            }
            let Ok(clip) = TempClip::create() else {
                complete_from_os(id, Err(VideoCaptureFailure::Internal));
                return;
            };
            let op = StorageFile::GetFileFromPathAsync(&HSTRING::from(clip.path().as_os_str()));
            let mut job = Job {
                id,
                max: Duration::from_secs(u64::from(max)),
                stage: Stage::Opening,
                clip: Some(clip),
                file: None,
                capture: None,
                failed_token: None,
                op: None,
                deadline: None,
                cancelling: false,
                result: None,
            };
            match spawn(op, &self.tx, move |r| {
                Cmd::File(id, r.and_then(|f| AgileReference::new(&f)))
            }) {
                Ok(op) => {
                    job.op = Some(op);
                    self.job = Some(job);
                }
                Err(e) => {
                    self.job = Some(job);
                    self.finish(id, Err(failure_from_hresult(e.code().0)));
                }
            }
        }

        fn after_file(&mut self, id: u64, r: windows::core::Result<AgileReference<StorageFile>>) {
            let tx = self.tx.clone();
            let Some(j) = self.cur(id) else { return };
            j.op = None;
            let file = match r.and_then(|f| f.resolve()) {
                Ok(f) => f,
                Err(e) => return self.finish(id, Err(failure_from_hresult(e.code().0))),
            };
            let started = (|| {
                let capture = MediaCapture::new()?;
                let settings = MediaCaptureInitializationSettings::new()?;
                // 映像のみ。音声は録らない（マイクの許可を求めない）。
                settings.SetStreamingCaptureMode(StreamingCaptureMode::Video)?;
                let op = capture.InitializeWithSettingsAsync(&settings);
                j.capture = Some(capture);
                spawn_action(op, &tx, move |r| Cmd::Initialized(id, r))
            })();
            j.file = Some(file);
            match started {
                Ok(op) => j.op = Some(op),
                Err(e) => self.finish(id, Err(failure_from_hresult(e.code().0))),
            }
        }

        fn after_init(&mut self, id: u64, r: windows::core::Result<()>) {
            let tx = self.tx.clone();
            let Some(j) = self.cur(id) else { return };
            j.op = None;
            if let Err(e) = r {
                return self.finish(id, Err(failure_from_hresult(e.code().0)));
            }
            let registered = match &j.capture {
                Some(c) => c.Failed(&MediaCaptureFailedEventHandler::new(move |_, _| {
                    let _ = tx.send(Cmd::CaptureFailed(id));
                    Ok(())
                })),
                None => Err(windows::core::Error::empty()),
            };
            match registered {
                Ok(t) => j.failed_token = Some(t),
                Err(_) => return self.finish(id, Err(VideoCaptureFailure::Internal)),
            }
            // 表示開始を記録できない要求（中断済み・古い）は成功値を返せないので、解放して終える。
            if !try_begin_present(id) {
                return self.finish(id, Err(VideoCaptureFailure::Interrupted));
            }
            j.stage = Stage::Ready;
            set_panel(id, Some(PanelPhase::Ready));
        }

        fn panel(&mut self, id: u64, c: PanelCommand) {
            let Some(stage) = self.cur(id).map(|j| j.stage) else {
                return;
            };
            match (stage, c) {
                (Stage::Ready, PanelCommand::Start) => self.start_recording(id),
                (Stage::Recording, PanelCommand::Stop) => self.stop_recording(id),
                (Stage::Ready, PanelCommand::Cancel) => {
                    self.finish(id, Ok(VideoCaptureOutcome::Cancelled))
                }
                (Stage::Recording, PanelCommand::Cancel) => {
                    if let Some(j) = self.cur(id) {
                        j.cancelling = true;
                    }
                    self.stop_recording(id);
                }
                // 状態に合わない命令（重複・遅れて届いた命令）は捨てる。
                _ => {}
            }
        }

        fn start_recording(&mut self, id: u64) {
            let tx = self.tx.clone();
            let Some(j) = self.cur(id) else { return };
            let started = (|| {
                let profile = MediaEncodingProfile::CreateMp4(VideoEncodingQuality::Auto)?;
                profile.SetAudio(None::<&AudioEncodingProperties>)?;
                let (Some(c), Some(f)) = (&j.capture, &j.file) else {
                    return Err(windows::core::Error::empty());
                };
                spawn_action(
                    c.StartRecordToStorageFileAsync(&profile, f),
                    &tx,
                    move |r| Cmd::Started(id, r),
                )
            })();
            match started {
                Ok(op) => {
                    j.op = Some(op);
                    j.stage = Stage::Starting;
                    set_panel(id, Some(PanelPhase::Busy));
                }
                Err(e) => self.finish(id, Err(failure_from_hresult(e.code().0))),
            }
        }

        fn after_started(&mut self, id: u64, r: windows::core::Result<()>) {
            let Some(j) = self.cur(id) else { return };
            j.op = None;
            if let Err(e) = r {
                return self.finish(id, Err(failure_from_hresult(e.code().0)));
            }
            // 期限は録画が始まった時点から単調時計で測る。
            j.deadline = Some(Instant::now() + j.max);
            j.stage = Stage::Recording;
            set_panel(id, Some(PanelPhase::Recording));
        }

        fn deadline_reached(&mut self) {
            let Some(j) = self.job.as_mut() else { return };
            j.deadline = None;
            if j.stage == Stage::Recording {
                let id = j.id;
                self.stop_recording(id);
            }
        }

        fn stop_recording(&mut self, id: u64) {
            let tx = self.tx.clone();
            let Some(j) = self.cur(id) else { return };
            j.deadline = None;
            let stopped = match &j.capture {
                Some(c) => spawn_action(c.StopRecordAsync(), &tx, move |r| Cmd::Stopped(id, r)),
                None => Err(windows::core::Error::empty()),
            };
            match stopped {
                Ok(op) => {
                    j.op = Some(op);
                    j.stage = Stage::Stopping;
                    set_panel(id, Some(PanelPhase::Busy));
                }
                Err(_) => self.finish(id, Err(VideoCaptureFailure::Internal)),
            }
        }

        fn after_stopped(&mut self, id: u64, r: windows::core::Result<()>) {
            let tx = self.tx.clone();
            let Some(j) = self.cur(id) else { return };
            j.op = None;
            if j.cancelling {
                // 取り消し。停止の成否に関わらず取り消しとして終える（解放の失敗だけが `Internal`）。
                return self.finish(id, Ok(VideoCaptureOutcome::Cancelled));
            }
            if let Err(e) = r {
                return self.finish(id, Err(failure_from_hresult(e.code().0)));
            }
            let reading = match &j.file {
                Some(f) => f.Properties().and_then(|p| {
                    spawn(p.GetVideoPropertiesAsync(), &tx, move |r| {
                        Cmd::Props(id, r.and_then(read_props))
                    })
                }),
                None => Err(windows::core::Error::empty()),
            };
            match reading {
                Ok(op) => {
                    j.op = Some(op);
                    j.stage = Stage::Reading;
                }
                Err(_) => {
                    // 0 バイトはメタデータが読めなくても Empty（数値表: 0 バイトは Empty）。
                    let empty = self
                        .cur(id)
                        .is_some_and(|j| j.clip.as_ref().and_then(|c| c.size().ok()) == Some(0));
                    let f = if empty {
                        VideoCaptureFailure::Empty
                    } else {
                        VideoCaptureFailure::Unreadable
                    };
                    self.finish(id, Err(f))
                }
            }
        }

        fn after_props(&mut self, id: u64, r: windows::core::Result<ClipProps>) {
            let Some(j) = self.cur(id) else { return };
            j.op = None;
            let size = j.clip.as_ref().and_then(|c| c.size().ok());
            let outcome = match (r, size) {
                (Ok(p), Some(size)) => VideoClip::from_os(
                    size,
                    duration_ms_from_ticks(p.ticks),
                    p.width,
                    p.height,
                    p.rotation,
                )
                .map(VideoCaptureOutcome::Captured),
                // 0 バイトはメタデータの成否によらず Empty。
                (_, Some(0)) => Err(VideoCaptureFailure::Empty),
                _ => Err(VideoCaptureFailure::Unreadable),
            };
            self.finish(id, outcome);
        }

        fn capture_failed(&mut self, id: u64) {
            if self.cur(id).is_some_and(|j| j.stage != Stage::Closing) {
                self.finish(id, Err(VideoCaptureFailure::Internal));
            }
        }

        /// 結果を決めて解放へ進む。保留中の操作があれば `Cancel` して、完了が届くまで待つ。
        fn finish(&mut self, id: u64, result: Outcome) {
            let Some(j) = self.cur(id) else { return };
            if j.stage == Stage::Closing {
                return;
            }
            j.stage = Stage::Closing;
            j.deadline = None;
            j.result = Some(result);
            set_panel(id, Some(PanelPhase::Busy));
            if let Some(op) = &j.op {
                // 戻り値は見ない。取り消しを含む完了が届いてから閉じる（契約表 11）。
                let _ = op.Cancel();
                return;
            }
            self.release();
        }

        /// 資源を解放し、結果を 1 回だけ返す。解放の失敗は読めた結果があっても `Internal` にする。
        fn release(&mut self) {
            let Some(mut j) = self.job.take() else { return };
            let mut ok = true;
            if let (Some(c), Some(t)) = (&j.capture, j.failed_token.take()) {
                ok &= c.RemoveFailed(t).is_ok();
            }
            drop(j.file.take());
            if let Some(c) = j.capture.take() {
                ok &= c.Close().is_ok();
            }
            if let Some(clip) = j.clip.take()
                && let Err(dir) = clip.remove()
            {
                self.leftovers.push(dir);
                ok = false;
            }
            let result = j
                .result
                .take()
                .unwrap_or(Err(VideoCaptureFailure::Internal));
            let result = match result {
                Ok(_) if !ok => Err(VideoCaptureFailure::Internal),
                r => r,
            };
            complete_from_os(j.id, result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hresult_maps_to_failure_kinds() {
        assert_eq!(
            failure_from_hresult(0x8007_0005_u32 as i32),
            VideoCaptureFailure::PermissionDenied
        );
        assert_eq!(
            failure_from_hresult(0xC00D_ABE0_u32 as i32),
            VideoCaptureFailure::Unsupported
        );
        assert_eq!(failure_from_hresult(0), VideoCaptureFailure::Internal);
        assert_eq!(
            failure_from_hresult(0x8000_4005_u32 as i32),
            VideoCaptureFailure::Internal
        );
    }

    #[test]
    fn ticks_become_milliseconds_and_never_zero() {
        assert_eq!(duration_ms_from_ticks(25_000_000), Some(2500));
        assert_eq!(duration_ms_from_ticks(0), None);
        assert_eq!(duration_ms_from_ticks(-5), None);
        assert_eq!(duration_ms_from_ticks(9_999), None);
    }

    /// テストごとの専用の置き場。テストは並行に走るため、置き場を共有すると
    /// 先に終わったテストの `remove_dir_all` が他のテストのファイルを消す（#1275）。
    fn base(test: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("fandhe-vc-test-{}-{test}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn temp_clip_is_fresh_per_request_and_removed() {
        let base = base("temp_clip_is_fresh_per_request_and_removed");
        let a = TempClip::create_in(&base).unwrap();
        let b = TempClip::create_in(&base).unwrap();
        assert_ne!(a.path(), b.path());
        assert_eq!(a.size().unwrap(), 0);
        assert!(a.path().exists());
        let dir = a.path().parent().unwrap().to_path_buf();
        assert!(a.remove().is_ok());
        assert!(!dir.exists());
        assert!(b.remove().is_ok());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn existing_directory_name_is_never_reused() {
        let base = base("existing_directory_name_is_never_reused");
        let a = TempClip::create_in(&base).unwrap();
        // 次に採番される名前を先に作っておく（差し替えの疑い）。
        let next = TEMP_SEQ.load(Ordering::Relaxed);
        let squatted = base.join(format!(
            "fandhe-video-capture-{}-{next}",
            std::process::id()
        ));
        fs::create_dir(&squatted).unwrap();
        let b = TempClip::create_in(&base).unwrap();
        assert_ne!(b.path().parent().unwrap(), squatted);
        assert!(!squatted.join("clip.mp4").exists());
        let _ = (a.remove(), b.remove());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn leftovers_are_retried_and_cleared() {
        let base = base("leftovers_are_retried_and_cleared");
        let dir = base.join("left");
        fs::create_dir(&dir).unwrap();
        let mut left = vec![dir.clone(), base.join("already-gone")];
        assert!(retry_leftovers(&mut left));
        assert!(left.is_empty());
        assert!(!dir.exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn zero_byte_clip_is_empty_not_success() {
        use crate::contract::VideoClip;
        let base = base("zero_byte_clip_is_empty_not_success");
        let c = TempClip::create_in(&base).unwrap();
        assert_eq!(
            VideoClip::from_os(c.size().unwrap(), Some(1000), 1920, 1080, 0),
            Err(VideoCaptureFailure::Empty)
        );
        let _ = c.remove();
        let _ = fs::remove_dir_all(&base);
    }
}
