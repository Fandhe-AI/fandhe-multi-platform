//! Android 向けフォルダ変更監視実装（Issue #1413）。Rust から inotify（`FileObserver` が包んでいる
//! のと同じカーネル機能）を直接呼び、アプリ専用の作業フォルダを監視する。`device/file_watch.rs` の
//! Android 経路（UI スレッド）から呼ばれ、契約は `fandhe_device::file_watch`。JNI・Java・権限は不要
//! （作業フォルダは内部ストレージ）。参考: <https://man7.org/linux/man-pages/man7/inotify.7.html>
//!
//! # 方式
//!
//! 監視 1 件につき専用スレッド 1 本が、停止用 eventfd と inotify fd を `poll` で待つ。イベントの
//! 中身（ファイル名）は読まず、「変化の合図」として `register` / `fire`（`device/file_watch.rs`）へ送る。
//! 停止（`AndroidGuard` の drop）は「登録解除 → eventfd 書き込み → join」の順で、drop が戻ると
//! 以後コールバックは走らない。Activity を作り直すと画面状態が初期化され、ドライバの照合が
//! 古い監視を drop する（遅れた合図も id 照合で捨てられる）。
//!
//! # 写像
//!
//! - `IN_Q_OVERFLOW`・空読み・欠けたヘッダは取りこぼしを意味するので `Changed`（再走査で吸収）
//! - 監視ルートの削除・移動・アンマウント（`IN_DELETE_SELF` / `IN_MOVE_SELF` / `IN_UNMOUNT` /
//!   `IN_IGNORED`）は `Failed`。開始失敗（inotify 上限 ENOSPC を含む）は `Internal("file_watch start")`
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗で登録表・fd・スレッドを残さない。パス・ファイル名・errno をログ・エラーに出さない
//! - 守らない範囲: 複数プロセス、同一ミリ秒の変化の順序、バックグラウンド中の遅れ

use fandhe_device::file_watch::WatchSignal;

// inotify の Linux ABI 定数（Android ビルドでは imp 内で libc の値との一致をコンパイル時に検査する）。
const IN_MODIFY: u32 = 0x2;
const IN_ATTRIB: u32 = 0x4;
const IN_CLOSE_WRITE: u32 = 0x8;
const IN_MOVED_FROM: u32 = 0x40;
const IN_MOVED_TO: u32 = 0x80;
const IN_CREATE: u32 = 0x100;
const IN_DELETE: u32 = 0x200;
const IN_DELETE_SELF: u32 = 0x400;
const IN_MOVE_SELF: u32 = 0x800;
const IN_UNMOUNT: u32 = 0x2000;
#[cfg(test)]
const IN_Q_OVERFLOW: u32 = 0x4000;
const IN_IGNORED: u32 = 0x8000;
const IN_ONLYDIR: u32 = 0x0100_0000;
const IN_DONT_FOLLOW: u32 = 0x0200_0000;

const HEADER: usize = 16;

/// `read` で得た inotify イベント列を合図へ写す純関数。ルート消失系が 1 つでもあれば `Failed`、
/// それ以外（あふれ・空・欠けを含む）は `Changed`。
fn classify(buf: &[u8]) -> WatchSignal {
    let fatal = IN_IGNORED | IN_DELETE_SELF | IN_MOVE_SELF | IN_UNMOUNT;
    let mut off = 0usize;
    while let Some(h) = buf.get(off..off.saturating_add(HEADER)) {
        let mask = u32::from_ne_bytes([h[4], h[5], h[6], h[7]]);
        let len = u32::from_ne_bytes([h[12], h[13], h[14], h[15]]) as usize;
        if mask & fatal != 0 {
            return WatchSignal::Failed;
        }
        off = off.saturating_add(HEADER).saturating_add(len);
    }
    WatchSignal::Changed
}

#[cfg(target_os = "android")]
mod imp {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use fandhe_device::file_watch::{FileWatch, OnSignal, WatchGuard, WatchSignal};
    use fandhe_device::DeviceError;

    use super::*;
    use crate::device::file_watch::{fire, register, unregister};

    const _: () = {
        assert!(IN_MODIFY == libc::IN_MODIFY);
        assert!(IN_ATTRIB == libc::IN_ATTRIB);
        assert!(IN_CLOSE_WRITE == libc::IN_CLOSE_WRITE);
        assert!(IN_MOVED_FROM == libc::IN_MOVED_FROM);
        assert!(IN_MOVED_TO == libc::IN_MOVED_TO);
        assert!(IN_CREATE == libc::IN_CREATE);
        assert!(IN_DELETE == libc::IN_DELETE);
        assert!(IN_DELETE_SELF == libc::IN_DELETE_SELF);
        assert!(IN_MOVE_SELF == libc::IN_MOVE_SELF);
        assert!(IN_UNMOUNT == libc::IN_UNMOUNT);
        assert!(IN_IGNORED == libc::IN_IGNORED);
        assert!(IN_ONLYDIR == libc::IN_ONLYDIR);
        assert!(IN_DONT_FOLLOW == libc::IN_DONT_FOLLOW);
    };

    /// 1 バッチで読む最大回数（4096 バイト × 16）。
    const MAX_READS: usize = 16;

    const FAILED: DeviceError = DeviceError::Internal("file_watch start");

    /// 新規 fd を直ちに所有物へ包む。負なら None。
    #[allow(unsafe_code)]
    fn own(fd: libc::c_int) -> Option<OwnedFd> {
        // SAFETY: fd は直前の呼び出しが返した、他の誰も所有していない有効な fd。
        (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn pollfd(fd: &OwnedFd) -> libc::pollfd {
        libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }
    }

    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    /// 監視スレッド本体。停止を優先し、`ino` はスレッド終了で drop され監視も解除される。
    #[allow(unsafe_code)]
    fn run(id: u64, ino: OwnedFd, stop: Arc<OwnedFd>) {
        let mut buf = [0u8; 4096];
        loop {
            let mut fds = [pollfd(&stop), pollfd(&ino)];
            // SAFETY: fds は有効な pollfd 2 個の配列で、呼び出しの間生きる。
            let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if r < 0 {
                if errno() == libc::EINTR {
                    continue;
                }
                fire(id, WatchSignal::Failed);
                return;
            }
            if fds[0].revents != 0 {
                return;
            }
            if fds[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                fire(id, WatchSignal::Failed);
                return;
            }
            if fds[1].revents & libc::POLLIN == 0 {
                continue;
            }
            // 非ブロッキング。継続書き込みでキューが空にならなくても停止要求と合図が止まらないよう、
            // 1 回の読み取りは MAX_READS 回までの有限バッチにし、合図を送って外側の poll（停止優先）へ戻る。
            let mut sig = None;
            for _ in 0..MAX_READS {
                // SAFETY: buf は有効で書き込み可能な 4096 バイト。長さも同じ値を渡す。
                let n = unsafe { libc::read(ino.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 {
                    match errno() {
                        libc::EINTR => continue,
                        libc::EAGAIN => {}
                        _ => sig = Some(WatchSignal::Failed),
                    }
                    break;
                }
                if n == 0 {
                    break;
                }
                let s = classify(&buf[..n as usize]);
                if sig != Some(WatchSignal::Failed) {
                    sig = Some(s);
                }
                if s == WatchSignal::Failed {
                    break;
                }
            }
            if let Some(s) = sig {
                fire(id, s);
                if s == WatchSignal::Failed {
                    return;
                }
            }
        }
    }

    struct AndroidGuard {
        id: u64,
        stop: Arc<OwnedFd>,
        thread: Option<JoinHandle<()>>,
    }

    impl WatchGuard for AndroidGuard {}

    impl Drop for AndroidGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // 先に登録を外し、以後の遅れた合図を捨てさせてからスレッドを起こして終了を待つ。
            unregister(self.id);
            let one = 1u64;
            // SAFETY: stop は有効な eventfd。one は 8 バイトのローカルで呼び出しの間生きる。
            let _ = unsafe { libc::write(self.stop.as_raw_fd(), (&one as *const u64).cast(), 8) };
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    struct AndroidFileWatch;

    impl FileWatch for AndroidFileWatch {
        #[allow(unsafe_code)]
        fn start(
            &self,
            dir: &Path,
            on_signal: OnSignal,
        ) -> Result<Box<dyn WatchGuard>, DeviceError> {
            let path = CString::new(dir.as_os_str().as_bytes()).map_err(|_| FAILED)?;
            // SAFETY: 引数は有効なフラグのみ。
            let ino = own(unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) })
                .ok_or(FAILED)?;
            let mask = IN_CREATE
                | IN_DELETE
                | IN_MODIFY
                | IN_ATTRIB
                | IN_CLOSE_WRITE
                | IN_MOVED_FROM
                | IN_MOVED_TO
                | IN_DELETE_SELF
                | IN_MOVE_SELF
                | IN_ONLYDIR
                | IN_DONT_FOLLOW;
            // SAFETY: ino は有効な inotify fd。path は NUL 終端で呼び出しの間生きる。
            if unsafe { libc::inotify_add_watch(ino.as_raw_fd(), path.as_ptr(), mask) } < 0 {
                return Err(FAILED);
            }
            // SAFETY: 引数は有効なフラグのみ。
            let stop = own(unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) })
                .ok_or(FAILED)?;
            let stop = Arc::new(stop);
            // 監視の追加は返る前に済ませてあるので、開始直後から基準走査までの変化もキューに残る。
            let id = register(on_signal);
            let s = Arc::clone(&stop);
            match std::thread::Builder::new()
                .name("fandhe-file-watch".into())
                .spawn(move || run(id, ino, s))
            {
                Ok(t) => Ok(Box::new(AndroidGuard {
                    id,
                    stop,
                    thread: Some(t),
                })),
                Err(_) => {
                    unregister(id);
                    Err(FAILED)
                }
            }
        }
    }

    /// Android の `FileWatch` 実装を返す（`device/file_watch.rs` の dispatch から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn FileWatch> {
        Arc::new(AndroidFileWatch)
    }
}

#[cfg(target_os = "android")]
pub(crate) use imp::implementation;

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(mask: u32, name_len: u32) -> Vec<u8> {
        let mut v = 0i32.to_ne_bytes().to_vec();
        v.extend(mask.to_ne_bytes());
        v.extend(0u32.to_ne_bytes());
        v.extend(name_len.to_ne_bytes());
        v.extend(std::iter::repeat_n(b'a', name_len as usize));
        v
    }

    #[test]
    fn classify_maps_events() {
        assert_eq!(classify(&ev(IN_CREATE, 16)), WatchSignal::Changed);
        assert_eq!(classify(&ev(IN_Q_OVERFLOW, 0)), WatchSignal::Changed);
        assert_eq!(classify(&ev(IN_IGNORED, 0)), WatchSignal::Failed);
        assert_eq!(classify(&ev(IN_DELETE_SELF, 0)), WatchSignal::Failed);
        assert_eq!(classify(&ev(IN_UNMOUNT, 0)), WatchSignal::Failed);
    }

    #[test]
    fn classify_walks_multiple_events_and_tolerates_garbage() {
        let mut b = ev(IN_CREATE, 16);
        b.extend(ev(IN_MOVE_SELF, 0));
        assert_eq!(classify(&b), WatchSignal::Failed);
        let mut b = ev(IN_CREATE, 8);
        b.extend(ev(IN_MODIFY, 0));
        assert_eq!(classify(&b), WatchSignal::Changed);
        assert_eq!(classify(&[]), WatchSignal::Changed);
        assert_eq!(classify(&[1, 2, 3]), WatchSignal::Changed);
        assert_eq!(classify(&ev(IN_CREATE, 16)[..20]), WatchSignal::Changed);
    }
}
