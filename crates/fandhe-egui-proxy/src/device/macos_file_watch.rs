//! macOS 向けフォルダ変更監視実装（Issue #1410）。`macos/fandhe_macos_file_watch.m`（CoreServices の
//! FSEventStream。`build.rs` が macOS 分岐で feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/file_watch.rs` の macOS 経路から UI スレッドで呼ばれ、契約は `fandhe_device::file_watch`。
//!
//! # 方式
//!
//! OS 資源は `id` で `.m` が所有し、Rust へは生ポインタを渡さない。変化は `.m` が
//! `fandhe_rust_file_watch_fire`（`device/file_watch.rs`）経由で `id` の登録表を引いて合図にする。
//! 停止（[`MacGuard`] の drop）は「登録解除 → `.m` の停止（実行中コールバックの終了を待つ）」の順。
//! 実パス（`canonicalize`）で開始する。`/var` → `/private/var` のような symlink 経由のパスでは
//! FSEvents が届かないため。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗で登録表と OS 資源を残さない。drop が戻った後にコールバックが走らない。
//!   パス・ファイル名をログ・エラーに出さない
//! - 守らない範囲: FSEvents の遅延（latency 0.2 秒）によるまとめ届け、複数プロセスからの同時監視

use std::ffi::CString;
use std::path::Path;
use std::sync::Arc;

use fandhe_device::file_watch::{FileWatch, OnSignal, WatchGuard};
use fandhe_device::DeviceError;

use super::file_watch::{register, unregister};

mod ffi {
    // SAFETY: `macos/fandhe_macos_file_watch.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_file_watch_start(id: u64, path: *const std::ffi::c_char) -> i32;
        fn fandhe_macos_file_watch_stop(id: u64);
    }

    /// 開始に成功したか。
    #[allow(unsafe_code)]
    pub(super) fn start(id: u64, path: &std::ffi::CStr) -> bool {
        // SAFETY: `path` は NUL 終端の有効な C 文字列で、呼び出しの間生きる。`.m` は保持せず
        // 内部で `NSString` へ複製する。
        unsafe { fandhe_macos_file_watch_start(id, path.as_ptr()) == 0 }
    }

    /// 停止して解放する。未知の `id` は何もしない。
    #[allow(unsafe_code)]
    pub(super) fn stop(id: u64) {
        // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。未知の id は no-op。
        unsafe { fandhe_macos_file_watch_stop(id) }
    }
}

/// 実行中の監視。drop で停止する。
struct MacGuard {
    id: u64,
}

impl WatchGuard for MacGuard {}

impl Drop for MacGuard {
    fn drop(&mut self) {
        // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから OS を止める（ここで待つ）。
        unregister(self.id);
        ffi::stop(self.id);
    }
}

struct MacFileWatch;

impl FileWatch for MacFileWatch {
    fn start(&self, dir: &Path, on_signal: OnSignal) -> Result<Box<dyn WatchGuard>, DeviceError> {
        const FAILED: DeviceError = DeviceError::Internal("file_watch start");
        let real = std::fs::canonicalize(dir).map_err(|_| FAILED)?;
        let c_path = CString::new(std::os::unix::ffi::OsStrExt::as_bytes(real.as_os_str()))
            .map_err(|_| FAILED)?;
        let id = register(on_signal);
        if !ffi::start(id, &c_path) {
            unregister(id);
            return Err(FAILED);
        }
        Ok(Box::new(MacGuard { id }))
    }
}

/// macOS の `FileWatch` 実装を返す（`device/file_watch.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn FileWatch> {
    Arc::new(MacFileWatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::file_watch::WatchSignal;
    use std::sync::mpsc;
    use std::time::Duration;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("fandhe-fw-mac-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
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
