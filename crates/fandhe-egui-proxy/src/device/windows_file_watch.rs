//! Windows 向けフォルダ変更監視実装（Issue #1411）。Win32 `ReadDirectoryChangesW`（overlapped I/O）で
//! アプリ専用の作業フォルダを監視する。`device/file_watch.rs` の Windows 経路から UI スレッドで呼ばれ、
//! 契約は `fandhe_device::file_watch`。COM は使わない。権限は不要（Win32 のファイル API で package identity 不要）。
//!
//! # 方式
//!
//! 通知バッファの中身（ファイル名）は読まず、完了を「変化の合図」1 種として `register` / `fire`
//! （`device/file_watch.rs`）へ送る。監視 1 件につき専用スレッド 1 本が、停止イベントと I/O 完了イベントを
//! `WaitForMultipleObjects` で待つ。バッファ・`OVERLAPPED`・ディレクトリ `HANDLE` はスレッドの外へ貸さない。
//! 停止（[`WinGuard`] の drop）は「登録解除 → 停止イベント → スレッド join」の順で、スレッドは発行中の I/O を
//! `CancelIoEx` した後に完了を待ってから資源を閉じる（カーネルが書き終える前に解放しない）。
//!
//! # 写像
//!
//! - 0 バイト完了・`ERROR_NOTIFY_ENUM_DIR`（バッファあふれ）は取りこぼしを意味するので `Changed`（再走査で吸収）
//! - フォルダ削除などの I/O 失敗は `Failed`。開始失敗は `Internal("file_watch start")`
//! - `LAST_WRITE` 通知はファイルのクローズ後に遅れて届くことがある（再走査で吸収。遅れの大きさは #1409 で実機確認）
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗で登録表・HANDLE・スレッドを残さない。drop が戻ると以後コールバックは走らない。
//!   パス・ファイル名・OS エラーコードをログ・エラーに出さない
//! - 守らない範囲: 通知の遅延による一時的な表示の食い違い、複数プロセスからの同時監視

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::thread::JoinHandle;

use fandhe_device::file_watch::{FileWatch, OnSignal, WatchGuard, WatchSignal};
use fandhe_device::DeviceError;
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_NOTIFY_ENUM_DIR, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
    FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, INFINITE,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use super::file_watch::{fire, register, unregister};

const FAILED: DeviceError = DeviceError::Internal("file_watch start");

/// 完了結果を合図へ写す純関数。`Ok`（0 バイト完了のあふれを含む）と `ERROR_NOTIFY_ENUM_DIR` は
/// `Changed`、それ以外の失敗は `Failed`。
fn classify(r: Result<(), HRESULT>) -> WatchSignal {
    match r {
        Ok(()) => WatchSignal::Changed,
        Err(c) if c == ERROR_NOTIFY_ENUM_DIR.to_hresult() => WatchSignal::Changed,
        Err(_) => WatchSignal::Failed,
    }
}

/// 監視スレッドが持つ資源。スレッド内でのみ生成・破棄する。
struct Watcher {
    dir: HANDLE,
    io: HANDLE,
    ov: Box<OVERLAPPED>,
    buf: Box<[u32; 256]>,
    pending: bool,
}

impl Watcher {
    #[allow(unsafe_code)]
    fn open(wide: &[u16]) -> Option<Self> {
        // SAFETY: `wide` は NUL 終端の UTF-16 で呼び出しの間生きる。戻り値の HANDLE は Drop で閉じる。
        let dir = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .ok()?;
        // SAFETY: 引数は有効な既定値。
        let io = match unsafe { CreateEventW(None, true, false, None) } {
            Ok(h) => h,
            Err(_) => {
                // SAFETY: 直前に開いた有効な HANDLE。
                let _ = unsafe { CloseHandle(dir) };
                return None;
            }
        };
        let ov = Box::new(OVERLAPPED {
            hEvent: io,
            ..Default::default()
        });
        Some(Self {
            dir,
            io,
            ov,
            buf: Box::new([0; 256]),
            pending: false,
        })
    }

    /// 次の通知待ちを発行する。発行できたら true。
    #[allow(unsafe_code)]
    fn issue(&mut self) -> bool {
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE;
        // SAFETY: `io` は有効なイベント。
        let _ = unsafe { ResetEvent(self.io) };
        // SAFETY: `dir` は overlapped で開いた有効なハンドル。`buf` / `ov` は Box でアドレスが固定され、
        // 完了（または Cancel 後の完了待ち）まで Watcher が生きて解放されない。
        let r = unsafe {
            ReadDirectoryChangesW(
                self.dir,
                self.buf.as_mut_ptr().cast(),
                std::mem::size_of_val(&*self.buf) as u32,
                false,
                filter,
                None,
                Some(&mut *self.ov),
                None,
            )
        };
        self.pending = match r {
            Ok(()) => true,
            Err(e) => e.code() == ERROR_IO_PENDING.to_hresult(),
        };
        self.pending
    }

    /// 完了済みの I/O 結果を取り出す。
    #[allow(unsafe_code)]
    fn result(&mut self, wait: bool) -> Result<(), HRESULT> {
        let mut n = 0u32;
        self.pending = false;
        // SAFETY: `dir` / `ov` は発行時と同じ有効なもの。`n` は呼び出しの間生きるローカル。
        unsafe { GetOverlappedResult(self.dir, &*self.ov, &mut n, wait) }.map_err(|e| e.code())
    }
}

impl Drop for Watcher {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.pending {
            // SAFETY: 発行中の I/O を取り消し、完了（中断を含む）を待ってからバッファを解放する。
            let _ = unsafe { CancelIoEx(self.dir, Some(&*self.ov)) };
            let _ = self.result(true);
        }
        // SAFETY: どちらも本構造体が所有する有効な HANDLE で、二重に閉じない。
        unsafe {
            let _ = CloseHandle(self.io);
            let _ = CloseHandle(self.dir);
        }
    }
}

/// 監視スレッド本体。起動結果を `ready` で返した後、停止まで合図を送り続ける。
/// `stop` は親が所有する停止イベント（ここでは閉じない）。
#[allow(unsafe_code)]
fn run(id: u64, wide: Vec<u16>, stop: usize, ready: std::sync::mpsc::SyncSender<bool>) {
    let stop = HANDLE(stop as *mut _);
    let Some(mut w) = Watcher::open(&wide) else {
        let _ = ready.send(false);
        return;
    };
    if !w.issue() {
        let _ = ready.send(false);
        return;
    }
    // 最初の発行を済ませてから返すので、start 後・基準走査前の変化も合図として届く。
    let _ = ready.send(true);
    loop {
        // SAFETY: 2 つとも有効なイベントで、呼び出しの間生きる。停止を先頭に置き同時なら停止を優先する。
        let r = unsafe { WaitForMultipleObjects(&[stop, w.io], false, INFINITE) };
        if r == WAIT_OBJECT_0 {
            return;
        }
        if r.0 != WAIT_OBJECT_0.0 + 1 {
            fire(id, WatchSignal::Failed);
            return;
        }
        let sig = classify(w.result(false));
        fire(id, sig);
        if sig == WatchSignal::Failed {
            return;
        }
        if !w.issue() {
            fire(id, WatchSignal::Failed);
            return;
        }
    }
}

/// 実行中の監視。drop で停止する。
struct WinGuard {
    id: u64,
    stop: HANDLE,
    thread: Option<JoinHandle<()>>,
}

impl WatchGuard for WinGuard {}

// SAFETY: `HANDLE` は停止イベントで、所有権は guard が持つ。スレッド間で触るのは SetEvent / CloseHandle のみ
// （いずれもスレッドセーフ）で、CloseHandle はワーカーの終了を join で待ってから行う。
// そのため別スレッドへ move しても、使用中のハンドルを閉じない。
// trait が `WatchGuard: Send` を要求するため明示する。
#[allow(unsafe_code)]
unsafe impl Send for WinGuard {}

impl Drop for WinGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから、スレッドを止めて終了を待つ。
        unregister(self.id);
        // SAFETY: `stop` は本構造体が所有する有効なイベント。
        let _ = unsafe { SetEvent(self.stop) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // SAFETY: スレッド終了後なので他に使う者はいない。二重に閉じない。
        let _ = unsafe { CloseHandle(self.stop) };
    }
}

struct WinFileWatch;

impl FileWatch for WinFileWatch {
    #[allow(unsafe_code)]
    fn start(&self, dir: &Path, on_signal: OnSignal) -> Result<Box<dyn WatchGuard>, DeviceError> {
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: 引数は有効な既定値（manual-reset・非シグナル）。
        let stop = unsafe { CreateEventW(None, true, false, None) }.map_err(|_| FAILED)?;
        let id = register(on_signal);
        let (tx, rx) = sync_channel::<bool>(1);
        let stop_raw = stop.0 as usize;
        let thread = std::thread::Builder::new()
            .name("fandhe-file-watch".into())
            .spawn(move || run(id, wide, stop_raw, tx));
        // 待つのはローカルの Win32 呼び出しだけなので期限は付けない（windows_system_tray と同じ）。
        let started = thread.is_ok() && rx.recv() == Ok(true);
        match thread {
            Ok(t) if started => Ok(Box::new(WinGuard {
                id,
                stop,
                thread: Some(t),
            })),
            thread => {
                unregister(id);
                if let Ok(t) = thread {
                    let _ = t.join();
                }
                // SAFETY: 本関数が作った有効なイベントで、スレッドは終了済み。
                let _ = unsafe { CloseHandle(stop) };
                Err(FAILED)
            }
        }
    }
}

/// Windows の `FileWatch` 実装を返す（`device/file_watch.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn FileWatch> {
    Arc::new(WinFileWatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Win32::Foundation::ERROR_ACCESS_DENIED;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("fandhe-fw-win-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn classify_maps_overflow_to_changed_and_errors_to_failed() {
        assert_eq!(classify(Ok(())), WatchSignal::Changed);
        assert_eq!(
            classify(Err(ERROR_NOTIFY_ENUM_DIR.to_hresult())),
            WatchSignal::Changed
        );
        assert_eq!(
            classify(Err(ERROR_ACCESS_DENIED.to_hresult())),
            WatchSignal::Failed
        );
    }

    #[test]
    fn real_os_delivers_changed_and_stops() {
        let dir = tmp("real");
        let (tx, rx) = mpsc::channel();
        let guard = implementation()
            .start(
                &dir,
                Arc::new(move |s| {
                    let _ = tx.send(s);
                }),
            )
            .expect("開始できる");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(WatchSignal::Changed)
        );
        drop(guard);
        while rx.try_recv().is_ok() {}
        std::fs::write(dir.join("b.txt"), "x").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_folder_fails_without_leaving_registration() {
        let dir = tmp("missing").join("nope");
        let r = implementation().start(&dir, Arc::new(|_| {}));
        assert!(matches!(r, Err(DeviceError::Internal("file_watch start"))));
    }
}
