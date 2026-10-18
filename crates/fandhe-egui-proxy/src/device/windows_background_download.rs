//! Windows 向けバックグラウンドダウンロード実装（Issue #394。親 #100）。BITS
//! （`IBackgroundCopyManager`）に固定 URL の転送 1 件を依頼する。`BackgroundDownloader` は
//! package identity が要るため使わない。BITS は開始したユーザーがログオンしている間、アプリが
//! 終了しても転送を続ける。
//!
//! `device/background_download.rs::implementation` の Windows 経路から [`implementation`] が
//! 呼ばれ、返った `fandhe_device::background_download::BackgroundDownload` を effect ドライバが
//! 別スレッドから `start` / `cancel`、UI スレッドから `status` で使う。dispatch と `device.rs` は
//! 触らない。macOS の `apple_background_download.rs` と同じ契約・同じ失敗文言で揃える。
//!
//! # 構造（契約表 11）
//!
//! 専用ワーカースレッド 1 本が COM（MTA）と BITS のジョブを持ち、`start` / `cancel` を直列キューで
//! 受けて、追跡中は 500ms ごとにジョブを 1 回ポーリングする。`status()` は共有状態の clone を返す
//! だけで COM を呼ばない（UI スレッドが STA の可能性があるため。`windows_background_task.rs` と同じ）。
//! 状態を書くのはワーカーだけで、OS からのコールバックは使わないため古い通知が届く経路が無く、
//! 世代番号は持たない（契約表 2）。`start` / `cancel` に期限は設けない（期限切れの後にワーカーが
//! OS の状態を変える窓を作らないため。契約表 3）。
//!
//! # 保存先
//!
//! `%LOCALAPPDATA%\fandhe-egui-proxy\` + `FILE_NAME`。サーバー由来のファイル名は使わない（A01）。
//! BITS は `Complete()` を呼ぶまで最終ファイルを作らないので、転送済みの検出時に呼んでからサイズを読む。
//!
//! # 再起動後の再接続（契約表 1）
//!
//! ワーカーは起動時に現在ユーザーのジョブを列挙し、表示名が `JOB_NAME` のものを拾う。未終端か
//! 転送済みの 1 件を追跡し、それ以外（エラー・余分な同名ジョブ）は取り消す。GUID は保存しない
//! （保存失敗という経路を作らない）。ponytail: 同名の他アプリのジョブは想定外。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の開始・取り消し・完了の順序、失敗を成功値へ畳まないこと、失敗経路での
//! ジョブと一時ファイルの解放、リダイレクトの拒否（`BG_HTTP_REDIRECT_POLICY_DISALLOW`。リダイレクト
//! 先の検証が要らない）。取り消しは `Cancelled` で失敗にしない。
//!
//! 守らない範囲: 起動直後の再接続前に UI が `Idle` を見て開始した場合は「置き換え」になる。接続できない
//! 間は BITS が再試行を続け `Running` のままになる（利用者は取り消しで止める）。ログオフ中は転送が
//! 止まる。BITS が一定期間で自動的に取り消したジョブは `Failed` になる。背景で失敗した理由は
//! 再起動で失われる。複数プロセス。
//!
//! `unsafe` は `com` サブモジュールの COM 呼び出しに限り、各呼び出しの直前に `// SAFETY:` を付ける
//! （`windows_background_task.rs` と同型）。

#[cfg(any(target_os = "windows", test))]
use std::ffi::OsString;
#[cfg(any(target_os = "windows", test))]
use std::path::PathBuf;
use std::sync::Arc;

use fandhe_device::background_download::BackgroundDownload;
#[cfg(any(target_os = "windows", test))]
use fandhe_device::background_download::{completion_status, DownloadStatus, FILE_NAME};
#[cfg(any(target_os = "windows", test))]
use fandhe_device::DeviceError;

/// BITS ジョブの表示名（再起動後の再接続に使う固定名）。
#[cfg(any(target_os = "windows", test))]
const JOB_NAME: &str = "fandhe-egui-proxy.background-download";

/// 保存先ディレクトリ名（`%LOCALAPPDATA%` 直下）。
#[cfg(any(target_os = "windows", test))]
const DIR_NAME: &str = "fandhe-egui-proxy";

// 失敗文言（Apple 実装と揃える。URL・パス・OS の生の文言は入れない。A09）。
#[cfg(any(target_os = "windows", test))]
const FAILED_REDIRECT: &str = "リダイレクト先が許可されていません";
#[cfg(any(target_os = "windows", test))]
const FAILED_NETWORK: &str = "接続できませんでした";
#[cfg(target_os = "windows")]
const FAILED_SAVE: &str = "ファイルを保存できませんでした";
#[cfg(target_os = "windows")]
const FAILED_OTHER: &str = "ダウンロードに失敗しました";
#[cfg(target_os = "windows")]
const FAILED_START: &str = "開始できませんでした";
#[cfg(target_os = "windows")]
const FAILED_CANCEL: &str = "保存したファイルを削除できませんでした";

/// `BG_SIZE_UNKNOWN`（総バイト数が不明）。
#[cfg(any(target_os = "windows", test))]
const SIZE_UNKNOWN: u64 = u64::MAX;

/// `BG_JOB_STATE` を自前の enum に写したもの（ホストでテストするため）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(target_os = "windows", test))]
enum JobState {
    /// 待機・接続・転送・一時停止・一時的なエラー（BITS が再試行する）。
    Active,
    /// 転送済み。`Complete()` でファイルを確定する前。
    Transferred,
    /// 回復不能なエラー。
    Error,
    /// 取り消し済み・確定済み・未知の値（自分の `cancel` 以外では成功にしない）。
    Dead,
}

#[cfg(any(target_os = "windows", test))]
impl JobState {
    /// `BG_JOB_STATE` の整数値から写す（0..=3 と 5 が未終端、4 がエラー、6 が転送済み）。
    fn from_raw(raw: i32) -> Self {
        match raw {
            0..=3 | 5 => Self::Active,
            4 => Self::Error,
            6 => Self::Transferred,
            _ => Self::Dead,
        }
    }
}

/// 未終端のジョブを `Running` に写す。総バイト数が不明なら `total: None`。
#[cfg(any(target_os = "windows", test))]
fn running(received: u64, total: u64) -> DownloadStatus {
    DownloadStatus::Running {
        received,
        total: (total != SIZE_UNKNOWN).then_some(total),
    }
}

/// ジョブのエラー HRESULT を状態へ写す。`0x8019xxxx` は HTTP の状態 `xxxx`（3xx はリダイレクトの
/// 拒否）、それ以外は接続失敗。
#[cfg(any(target_os = "windows", test))]
fn error_status(hresult: i32) -> DownloadStatus {
    let hr = hresult as u32;
    if hr & 0xFFFF_0000 == 0x8019_0000 {
        let code = (hr & 0xFFFF) as u16;
        if (300..400).contains(&code) {
            return DownloadStatus::Failed(FAILED_REDIRECT.to_string());
        }
        return completion_status(Some(code), 0);
    }
    DownloadStatus::Failed(FAILED_NETWORK.to_string())
}

/// COM / BITS の HRESULT を契約上のエラー種別へ写す（全呼び出し箇所がここを通る。契約表 5）。
#[cfg(any(target_os = "windows", test))]
fn map_hresult(hresult: i32) -> DeviceError {
    match hresult as u32 {
        0x8007_0005 => DeviceError::PermissionDenied("background download"),
        // サービス無効（ERROR_SERVICE_DISABLED）・クラス未登録（BITS が無い環境）。
        0x8007_0422 | 0x8004_0154 => DeviceError::Unsupported("background download"),
        _ => DeviceError::Internal("background download"),
    }
}

/// `%LOCALAPPDATA%` の値から保存先を組み立てる。未設定・相対パスなら失敗（相対のまま使わない）。
#[cfg(any(target_os = "windows", test))]
fn download_path(local_app_data: Option<OsString>) -> Result<PathBuf, DeviceError> {
    let base = PathBuf::from(local_app_data.ok_or(DeviceError::Internal("background download"))?);
    if !base.is_absolute() {
        return Err(DeviceError::Internal("background download"));
    }
    Ok(base.join(DIR_NAME).join(FILE_NAME))
}

/// 起動時の再接続（ジョブ列挙）がワーカーで終わったか。終わるまで画面側は継続ポーリングする
/// （ワーカーから再描画を起こせないため。契約表 4）。
#[cfg(any(target_os = "windows", test))]
static ENUMERATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 列挙の結果を反映する。失敗中は待ち（`pending`）を保ち、既存ジョブを把握できてから完了にする。
/// 開始要求が既存の転送を取り消して置き換えるのを防ぐ（契約表 2・4）。
#[cfg(any(target_os = "windows", test))]
fn mark_enumerated(enum_failed: bool) {
    if !enum_failed {
        ENUMERATED.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// 列挙待ちか。画面ドライバ（`background_download.rs`）が `drive_poll` の継続判定と開始要求の保留に使う。
#[cfg(any(target_os = "windows", test))]
pub(crate) fn enumeration_pending() -> bool {
    cfg!(target_os = "windows") && !ENUMERATED.load(std::sync::atomic::Ordering::Acquire)
}

/// Windows の実装を返す。ワーカーを 1 本立てる。BITS を使えない環境では各メソッドが `Unsupported`。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Arc<dyn BackgroundDownload> {
    Arc::new(worker::WindowsBackgroundDownload::spawn())
}

/// ホストのテストビルド用（Windows 以外では `Unsupported`）。
#[cfg(all(test, not(target_os = "windows")))]
pub(crate) fn implementation() -> Arc<dyn BackgroundDownload> {
    Arc::new(fandhe_device::background_download::UnsupportedBackgroundDownload)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod com {
    //! BITS の COM 呼び出し。ジョブは呼び出したワーカースレッドの中だけで持つ。

    use std::path::Path;

    use windows::core::{Interface, Result as WinResult, GUID, HRESULT, HSTRING};
    use windows::Win32::Foundation::E_POINTER;
    use windows::Win32::Networking::BackgroundIntelligentTransferService::{
        BackgroundCopyManager, IBackgroundCopyJob, IBackgroundCopyJobHttpOptions,
        IBackgroundCopyManager, BG_ERROR_CONTEXT, BG_HTTP_REDIRECT_POLICY_DISALLOW,
        BG_JOB_PRIORITY_FOREGROUND, BG_JOB_PROGRESS, BG_JOB_TYPE_DOWNLOAD,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_LOCAL_SERVER};

    /// BITS マネージャーを得る（失敗は BITS を使えない環境）。
    pub(super) fn manager() -> WinResult<IBackgroundCopyManager> {
        // SAFETY: `BackgroundCopyManager` は BITS の CLSID。呼び出しスレッドは `ComInit` で
        // MTA 初期化済みで、戻り値は参照カウント付きのスマートポインタ。
        unsafe { CoCreateInstance(&BackgroundCopyManager, None, CLSCTX_LOCAL_SERVER) }
    }

    /// ジョブを作って転送を始める。設定に失敗したら、取り消しのためジョブも返す
    /// （呼び出し側が取り消し、失敗したら保持して再試行する。契約表 9）。
    pub(super) fn start(
        mgr: &IBackgroundCopyManager,
        name: &str,
        url: &str,
        path: &Path,
    ) -> Result<IBackgroundCopyJob, (windows::core::Error, Option<IBackgroundCopyJob>)> {
        let mut id = GUID::zeroed();
        let mut job: Option<IBackgroundCopyJob> = None;
        // SAFETY: `id` と `job` は呼び出しの間生きている出力先。`HSTRING` は呼び出し中有効。
        unsafe {
            mgr.CreateJob(
                &HSTRING::from(name),
                BG_JOB_TYPE_DOWNLOAD,
                &mut id,
                &mut job,
            )
            .map_err(|e| (e, None))?
        };
        let job = job.ok_or_else(|| (windows::core::Error::from(E_POINTER), None))?;
        let configure = || -> WinResult<()> {
            // SAFETY: `job` は有効な COM ポインタ。`cast` は QueryInterface。
            unsafe {
                // リダイレクトを拒否する（リダイレクト先の検証そのものが要らなくなる）。
                job.cast::<IBackgroundCopyJobHttpOptions>()?
                    .SetSecurityFlags(BG_HTTP_REDIRECT_POLICY_DISALLOW)?;
                job.SetPriority(BG_JOB_PRIORITY_FOREGROUND)?;
                job.AddFile(&HSTRING::from(url), &HSTRING::from(path.as_os_str()))?;
                job.Resume()
            }
        };
        match configure() {
            Ok(()) => Ok(job),
            Err(e) => Err((e, Some(job))),
        }
    }

    /// 現在ユーザーのジョブのうち、表示名が `name` のもの。
    pub(super) fn find(
        mgr: &IBackgroundCopyManager,
        name: &str,
    ) -> WinResult<Vec<IBackgroundCopyJob>> {
        // SAFETY: 引数 0 は現在ユーザーのジョブの列挙。
        let jobs = unsafe { mgr.EnumJobs(0)? };
        let mut found = Vec::new();
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            // SAFETY: `slot` と `fetched` は呼び出しの間生きている出力先。
            unsafe { jobs.Next(&mut slot, &mut fetched)? };
            let [Some(job)] = slot else { break };
            // SAFETY: `GetDisplayName` が返す PWSTR は呼び出し側が `CoTaskMemFree` で解放する。
            let display = unsafe {
                let p = job.GetDisplayName()?;
                let s = p.to_string();
                CoTaskMemFree(Some(p.0 as *const _));
                s.unwrap_or_default()
            };
            if display == name {
                found.push(job);
            }
        }
        Ok(found)
    }

    /// `(BG_JOB_STATE の整数値, 受信済み, 総バイト数)`。
    pub(super) fn observe(job: &IBackgroundCopyJob) -> WinResult<(i32, u64, u64)> {
        let mut p = BG_JOB_PROGRESS {
            BytesTotal: 0,
            BytesTransferred: 0,
            FilesTotal: 0,
            FilesTransferred: 0,
        };
        // SAFETY: `p` は呼び出しの間生きている出力先。
        unsafe {
            let state = job.GetState()?;
            job.GetProgress(&mut p)?;
            Ok((state.0, p.BytesTransferred, p.BytesTotal))
        }
    }

    /// エラー状態のジョブの HRESULT。取れなければ 0（接続失敗扱いになる）。
    pub(super) fn error_code(job: &IBackgroundCopyJob) -> i32 {
        let mut ctx = BG_ERROR_CONTEXT(0);
        let mut hr = HRESULT(0);
        // SAFETY: `ctx` と `hr` は呼び出しの間生きている出力先。
        let got = unsafe { job.GetError().and_then(|e| e.GetError(&mut ctx, &mut hr)) };
        if got.is_ok() {
            hr.0
        } else {
            0
        }
    }

    /// 転送済みファイルを確定する（呼ぶまで最終ファイルは使えない）。
    pub(super) fn complete(job: &IBackgroundCopyJob) -> WinResult<()> {
        // SAFETY: `job` は有効な COM ポインタ。
        unsafe { job.Complete() }
    }

    /// 一時停止中のジョブを再開する。
    pub(super) fn resume(job: &IBackgroundCopyJob) -> WinResult<()> {
        // SAFETY: `job` は有効な COM ポインタ。
        unsafe { job.Resume() }
    }

    /// ジョブを取り消す（BITS の一時ファイルも消える）。
    pub(super) fn cancel(job: &IBackgroundCopyJob) -> WinResult<()> {
        // SAFETY: `job` は有効な COM ポインタ。
        unsafe { job.Cancel() }
    }
}

#[cfg(target_os = "windows")]
mod worker {
    //! BITS を駆動するワーカーと、それを包む `BackgroundDownload` 実装。

    use std::path::Path;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use fandhe_device::background_download::{
        completion_status, BackgroundDownload, DownloadStatus,
    };
    use fandhe_device::DeviceError;
    use windows::Win32::Networking::BackgroundIntelligentTransferService::{
        IBackgroundCopyJob, IBackgroundCopyManager, BG_E_INVALID_STATE, BG_E_NOT_FOUND,
    };

    use super::{
        com, download_path, error_status, map_hresult, mark_enumerated, running, JobState,
        ENUMERATED, FAILED_CANCEL, FAILED_OTHER, FAILED_SAVE, FAILED_START, JOB_NAME, SIZE_UNKNOWN,
    };

    /// 追跡中のジョブをポーリングする間隔。
    const POLL_INTERVAL: Duration = Duration::from_millis(500);

    type Reply = Sender<Result<(), DeviceError>>;

    enum Cmd {
        Start(String, Reply),
        Cancel(Reply),
    }

    /// ワーカーと `status()` が共有する状態。
    struct Shared {
        status: DownloadStatus,
        /// BITS を使えない（初期化失敗）。初期化が終わるまでは `false`。
        unsupported: bool,
    }

    pub(super) struct WindowsBackgroundDownload {
        tx: Mutex<Sender<Cmd>>,
        shared: Arc<Mutex<Shared>>,
    }

    impl WindowsBackgroundDownload {
        pub(super) fn spawn() -> Self {
            let (tx, rx) = mpsc::channel();
            let shared = Arc::new(Mutex::new(Shared {
                status: DownloadStatus::Idle,
                unsupported: false,
            }));
            let s = Arc::clone(&shared);
            // 起動に失敗しても `tx` の送信が失敗するだけで、各メソッドは `Internal` を返す。
            // 列挙は終わらないので、画面が継続ポーリングしないよう終了済みにする。
            if std::thread::Builder::new()
                .name("fandhe-background-download".into())
                .spawn(move || run(rx, s))
                .is_err()
            {
                ENUMERATED.store(true, Ordering::Release);
            }
            Self {
                tx: Mutex::new(tx),
                shared,
            }
        }

        /// コマンドを送って返事を待つ。ワーカーが死んでいれば `Internal`。
        fn call(&self, make: impl FnOnce(Reply) -> Cmd) -> Result<(), DeviceError> {
            let dead = DeviceError::Internal("background download");
            let (reply_tx, reply_rx) = mpsc::channel();
            self.tx
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(make(reply_tx))
                .map_err(|_| DeviceError::Internal("background download"))?;
            reply_rx.recv().map_err(|_| dead)?
        }
    }

    impl BackgroundDownload for WindowsBackgroundDownload {
        fn start(&self, url: &str) -> Result<(), DeviceError> {
            fandhe_device::http::check_url(url)?;
            self.call(|r| Cmd::Start(url.to_string(), r))
        }
        fn cancel(&self) -> Result<(), DeviceError> {
            self.call(Cmd::Cancel)
        }
        fn status(&self) -> Result<DownloadStatus, DeviceError> {
            let g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            if g.unsupported {
                return Err(DeviceError::Unsupported("background download"));
            }
            Ok(g.status.clone())
        }
    }

    struct Worker {
        /// `None` は BITS を使えない環境（各操作が `Unsupported`）。
        mgr: Option<IBackgroundCopyManager>,
        tracked: Option<IBackgroundCopyJob>,
        /// 取り消しに失敗して BITS に残っている追跡外のジョブ。次のポーリングと `cancel` で再試行する。
        stale: Vec<IBackgroundCopyJob>,
        /// 直近の再接続の列挙に失敗した。既存ジョブを把握できるまで開始・取り消しを成功にしない。
        enum_failed: bool,
        shared: Arc<Mutex<Shared>>,
    }

    fn run(rx: mpsc::Receiver<Cmd>, shared: Arc<Mutex<Shared>>) {
        // COM のガードはジョブより後に解放する（先に宣言し、`w` を先に drop する）。
        let com_guard = super::super::windows_notification::ComInit::new("background download");
        let mut w = Worker {
            mgr: com_guard.is_ok().then(|| com::manager().ok()).flatten(),
            tracked: None,
            stale: Vec::new(),
            enum_failed: false,
            shared,
        };
        w.reconnect();
        // 初期化結果は列挙の完了より先に公開する（完了を見た画面が Idle を見ないように）。
        w.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unsupported = w.mgr.is_none();
        mark_enumerated(w.enum_failed);
        loop {
            let cmd = if w.tracked.is_some() || !w.stale.is_empty() || w.enum_failed {
                match rx.recv_timeout(POLL_INTERVAL) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => break,
                }
            };
            match cmd {
                Some(Cmd::Start(url, reply)) => {
                    let _ = reply.send(w.start(&url));
                }
                Some(Cmd::Cancel(reply)) => {
                    let _ = reply.send(w.cancel());
                }
                None => {}
            }
            w.poll();
            // 列挙の再試行が成功したらここで完了にする。
            mark_enumerated(w.enum_failed);
        }
        drop(w);
        drop(com_guard);
    }

    fn local_app_data() -> Result<std::path::PathBuf, DeviceError> {
        download_path(std::env::var_os("LOCALAPPDATA"))
    }

    impl Worker {
        fn set(&self, s: DownloadStatus) {
            self.shared
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .status = s;
        }

        /// ジョブを取り消す。BITS から消えた（取り消し済み・終端・不在）なら `true`。
        /// それ以外の失敗は `false` で、呼び出し側が追跡か `stale` に残して再試行する（契約表 9）。
        fn release(job: &IBackgroundCopyJob) -> bool {
            match com::cancel(job) {
                Ok(()) => true,
                Err(e) => matches!(e.code().0, BG_E_INVALID_STATE | BG_E_NOT_FOUND),
            }
        }

        /// 取り消せなかった追跡外ジョブを再試行する。残りがあれば `false`。
        fn drain_stale(&mut self) -> bool {
            self.stale.retain(|j| !Self::release(j));
            self.stale.is_empty()
        }

        fn failed(&self, msg: &str) {
            self.set(DownloadStatus::Failed(msg.to_string()));
        }

        /// 起動時に自分のジョブを拾う。未終端か転送済みの 1 件を追跡し、残りは取り消す。
        fn reconnect(&mut self) {
            self.scan();
            self.poll();
        }

        /// 列挙の再試行。成功するまで `enum_failed` を保つ。
        fn scan(&mut self) {
            let Some(mgr) = &self.mgr else { return };
            let Ok(jobs) = com::find(mgr, JOB_NAME) else {
                self.enum_failed = true;
                return;
            };
            self.enum_failed = false;
            for job in jobs {
                let raw = com::observe(&job).ok().map(|(s, _, _)| s);
                let live = self.tracked.is_none()
                    && raw.is_some_and(|s| {
                        matches!(
                            JobState::from_raw(s),
                            JobState::Active | JobState::Transferred
                        )
                    });
                if live {
                    // 一時停止(3)のまま拾うと動かないので再開する。失敗しても追跡は続ける。
                    if raw == Some(3) {
                        let _ = com::resume(&job);
                    }
                    self.set(running(0, SIZE_UNKNOWN));
                    self.tracked = Some(job);
                } else if !Self::release(&job) {
                    self.stale.push(job);
                }
            }
        }

        /// 既存ジョブを把握できていて、取り消せなかった旧ジョブも残っていなければ `Ok`。
        fn ensure_settled(&mut self) -> Result<(), DeviceError> {
            if self.enum_failed {
                self.scan();
            }
            if self.enum_failed || !self.drain_stale() {
                return Err(DeviceError::Internal("background download"));
            }
            Ok(())
        }

        fn start(&mut self, url: &str) -> Result<(), DeviceError> {
            let result = self.start_inner(url);
            // 旧ジョブが追跡に残っている（転送継続中の）経路では Failed を書かない。
            if result.is_err() && self.tracked.is_none() {
                self.failed(FAILED_START);
            }
            result
        }

        fn start_inner(&mut self, url: &str) -> Result<(), DeviceError> {
            let mgr = self
                .mgr
                .clone()
                .ok_or(DeviceError::Unsupported("background download"))?;
            // 取り消せない旧ジョブや未把握のジョブが残ると同名・同保存先が二重になるので開始しない。
            self.ensure_settled()?;
            let path = local_app_data()?;
            // 置き換え: 前の転送と前回のファイルを消してから始める。
            if let Some(old) = self.tracked.take() {
                if let Err(e) = com::cancel(&old) {
                    // 止まっていない旧ジョブが同じ保存先へ書き続けるので、追跡を戻して失敗にする。
                    let code = e.code().0;
                    if code != BG_E_INVALID_STATE && code != BG_E_NOT_FOUND {
                        self.tracked = Some(old);
                        return Err(map_hresult(code));
                    }
                }
            }
            remove_file(&path)?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let job = match com::start(&mgr, JOB_NAME, url, &path) {
                Ok(j) => j,
                Err((e, leftover)) => {
                    if let Some(j) = leftover {
                        if !Self::release(&j) {
                            self.stale.push(j);
                        }
                    }
                    return Err(map_hresult(e.code().0));
                }
            };
            self.tracked = Some(job);
            // 結果を返す前に状態を確定する（契約表 2）。
            self.set(running(0, SIZE_UNKNOWN));
            Ok(())
        }

        fn cancel(&mut self) -> Result<(), DeviceError> {
            if self.mgr.is_none() {
                return Err(DeviceError::Unsupported("background download"));
            }
            self.ensure_settled()?;
            if let Some(job) = self.tracked.take() {
                if let Err(e) = com::cancel(&job) {
                    // ジョブが既に無い・終端なら取り消し済みと同じ。それ以外は止まっていない。
                    let code = e.code().0;
                    if code != BG_E_INVALID_STATE && code != BG_E_NOT_FOUND {
                        self.tracked = Some(job);
                        return Err(map_hresult(code));
                    }
                }
            }
            if let Err(e) = local_app_data().and_then(|p| remove_file(&p)) {
                self.failed(FAILED_CANCEL);
                return Err(e);
            }
            self.set(DownloadStatus::Cancelled);
            Ok(())
        }

        /// 追跡中のジョブを 1 回読み、状態を更新する。終端に達したら追跡を外す。
        fn poll(&mut self) {
            if self.enum_failed {
                self.scan();
            }
            self.drain_stale();
            let Some(job) = self.tracked.clone() else {
                return;
            };
            let (raw, received, total) = match com::observe(&job) {
                Ok(v) => v,
                // ジョブが消えていれば失敗。BITS の再起動などの一時的な RPC エラーは次回に再試行する。
                Err(e) if e.code().0 == BG_E_NOT_FOUND => {
                    self.tracked = None;
                    self.failed(FAILED_OTHER);
                    return;
                }
                Err(_) => return,
            };
            match JobState::from_raw(raw) {
                JobState::Active => {
                    // 外部要因で一時停止(3)されたジョブは進まないので再開する。失敗しても次回に再試行する。
                    if raw == 3 {
                        let _ = com::resume(&job);
                    }
                    self.set(running(received, total));
                }
                JobState::Transferred => self.finish_transferred(&job),
                JobState::Error => {
                    let status = error_status(com::error_code(&job));
                    // 取り消せなければ追跡を残し、次のポーリングで再試行する。
                    if Self::release(&job) {
                        self.tracked = None;
                        self.set(status);
                    }
                }
                // 外部から取り消された。成功にしない。
                JobState::Dead => {
                    self.tracked = None;
                    self.failed(FAILED_OTHER);
                }
            }
        }

        fn finish_failed(&mut self, job: &IBackgroundCopyJob, msg: &str) {
            if Self::release(job) {
                self.tracked = None;
                self.failed(msg);
            }
        }

        fn finish_transferred(&mut self, job: &IBackgroundCopyJob) {
            let size = com::complete(job).ok().and_then(|()| {
                local_app_data()
                    .ok()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len())
            });
            let Some(bytes) = size else {
                // Complete 済みでサイズ取得だけ失敗した場合も、確定済みファイルを残さない。
                if let Ok(p) = local_app_data() {
                    let _ = remove_file(&p);
                }
                self.finish_failed(job, FAILED_SAVE);
                return;
            };
            self.tracked = None;
            let status = completion_status(None, bytes);
            if !matches!(status, DownloadStatus::Completed { .. }) {
                if let Ok(p) = local_app_data() {
                    let _ = remove_file(&p);
                }
            }
            self.set(status);
        }
    }

    /// `NotFound` は成功として扱う削除。
    fn remove_file(path: &Path) -> Result<(), DeviceError> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn 列挙失敗中は列挙完了にしない() {
        use std::sync::atomic::Ordering;
        super::ENUMERATED.store(false, Ordering::Release);
        super::mark_enumerated(true);
        assert!(!super::ENUMERATED.load(Ordering::Acquire));
        super::mark_enumerated(false);
        assert!(super::ENUMERATED.load(Ordering::Acquire));
    }

    use super::*;

    #[test]
    fn job_state_never_treats_cancelled_as_success() {
        assert_eq!(JobState::from_raw(0), JobState::Active);
        assert_eq!(JobState::from_raw(2), JobState::Active);
        assert_eq!(JobState::from_raw(5), JobState::Active);
        assert_eq!(JobState::from_raw(4), JobState::Error);
        assert_eq!(JobState::from_raw(6), JobState::Transferred);
        assert_eq!(JobState::from_raw(7), JobState::Dead);
        assert_eq!(JobState::from_raw(8), JobState::Dead);
        assert_eq!(JobState::from_raw(99), JobState::Dead);
    }

    #[test]
    fn unknown_total_becomes_none() {
        assert_eq!(
            running(10, SIZE_UNKNOWN),
            DownloadStatus::Running {
                received: 10,
                total: None
            }
        );
        assert_eq!(
            running(10, 40),
            DownloadStatus::Running {
                received: 10,
                total: Some(40)
            }
        );
    }

    #[test]
    fn error_status_maps_http_and_others() {
        // 0x80190194 = HTTP 404
        assert_eq!(
            error_status(0x8019_0194_u32 as i32),
            DownloadStatus::Failed("HTTP 404".into())
        );
        assert_eq!(
            error_status(0x8019_01F4_u32 as i32),
            DownloadStatus::Failed("HTTP 500".into())
        );
        assert_eq!(
            error_status(0x8019_012D_u32 as i32),
            DownloadStatus::Failed(FAILED_REDIRECT.into())
        );
        assert_eq!(
            error_status(0x8020_0001_u32 as i32),
            DownloadStatus::Failed(FAILED_NETWORK.into())
        );
        assert_eq!(
            error_status(0),
            DownloadStatus::Failed(FAILED_NETWORK.into())
        );
    }

    #[test]
    fn map_hresult_variants() {
        assert!(matches!(
            map_hresult(0x8007_0005_u32 as i32),
            DeviceError::PermissionDenied(_)
        ));
        assert!(matches!(
            map_hresult(0x8007_0422_u32 as i32),
            DeviceError::Unsupported(_)
        ));
        assert!(matches!(
            map_hresult(0x8004_0154_u32 as i32),
            DeviceError::Unsupported(_)
        ));
        assert!(matches!(map_hresult(-1), DeviceError::Internal(_)));
    }

    #[test]
    fn download_path_requires_absolute_base() {
        assert!(download_path(None).is_err());
        assert!(download_path(Some(OsString::from("relative/dir"))).is_err());
        let abs = if cfg!(windows) {
            "C:\\Users\\x\\AppData\\Local"
        } else {
            "/tmp/x"
        };
        let p = download_path(Some(OsString::from(abs))).unwrap();
        assert!(p.ends_with(PathBuf::from(DIR_NAME).join(FILE_NAME)));
    }

    #[test]
    fn host_implementation_is_unsupported() {
        if !cfg!(windows) {
            assert!(matches!(
                implementation().status(),
                Err(DeviceError::Unsupported(_))
            ));
        }
    }
}
