//! iOS 向けフォルダ変更監視実装（Issue #1412）。`ios/fandhe_ios_file_watch.m`（`open(O_EVTONLY)` +
//! `DISPATCH_SOURCE_TYPE_VNODE`。`build.rs` が iOS 分岐で feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/file_watch.rs` の iOS 経路から UI スレッドで呼ばれ、契約は `fandhe_device::file_watch`。
//!
//! # 方式
//!
//! OS 資源は `id` で `.m` が所有し、Rust へは生ポインタを渡さない。変化は `.m` が
//! `fandhe_rust_file_watch_fire`（`device/file_watch.rs`）経由で `id` の登録表を引いて合図にする。
//! 合図の直後に `crate::ios::request_repaint` で次フレームを起こす（入力がないと egui が描画せず
//! 画面が止まるため）。停止（[`IosGuard`] の drop）は「登録解除 → `.m` の停止（cancel 完了を待つ）」の順。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 開始失敗で登録表と OS 資源を残さない。drop が戻った後にコールバックが走らない。
//!   パス・ファイル名をログ・エラーに出さない
//! - 守らない範囲: ディレクトリの vnode に出ない既存ファイルの中身だけの書き換え（アプリ自身の更新は
//!   rename 経由で検出できる）、バックグラウンド中の遅れ、複数プロセス

#[cfg(target_os = "ios")]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::Arc;

    use fandhe_device::file_watch::{FileWatch, OnSignal, WatchGuard};
    use fandhe_device::DeviceError;

    use super::super::file_watch::{register, unregister};

    mod ffi {
        // SAFETY: `ios/fandhe_ios_file_watch.m`（`build.rs` が iOS 分岐で feature `device` 有効時に
        // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
        #[allow(unsafe_code)]
        extern "C" {
            fn fandhe_ios_file_watch_start(id: u64, path: *const std::ffi::c_char) -> i32;
            fn fandhe_ios_file_watch_stop(id: u64);
        }

        /// 開始に成功したか。
        #[allow(unsafe_code)]
        pub(in super::super) fn start(id: u64, path: &std::ffi::CStr) -> bool {
            // SAFETY: `path` は NUL 終端の有効な C 文字列で、呼び出しの間生きる。`.m` は `open` の間だけ
            // 読み、保持しない。
            unsafe { fandhe_ios_file_watch_start(id, path.as_ptr()) == 0 }
        }

        /// 停止して解放する。未知の `id` は何もしない。
        #[allow(unsafe_code)]
        pub(in super::super) fn stop(id: u64) {
            // SAFETY: `id` は値渡しで、`.m` は自前の辞書だけを引く。未知の id は no-op。
            unsafe { fandhe_ios_file_watch_stop(id) }
        }
    }

    /// 実行中の監視。drop で停止する。
    struct IosGuard {
        id: u64,
    }

    impl WatchGuard for IosGuard {}

    impl Drop for IosGuard {
        fn drop(&mut self) {
            // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから OS を止める（ここで待つ）。
            unregister(self.id);
            ffi::stop(self.id);
        }
    }

    struct IosFileWatch;

    impl FileWatch for IosFileWatch {
        fn start(
            &self,
            dir: &Path,
            on_signal: OnSignal,
        ) -> Result<Box<dyn WatchGuard>, DeviceError> {
            const FAILED: DeviceError = DeviceError::Internal("file_watch start");
            // バイト列のまま渡す（lossy 変換しない）。NUL を含めば失敗。
            let c_path = CString::new(dir.as_os_str().as_bytes()).map_err(|_| FAILED)?;
            let id = register(Arc::new(move |s| {
                on_signal(s);
                crate::ios::request_repaint();
            }));
            if !ffi::start(id, &c_path) {
                unregister(id);
                return Err(FAILED);
            }
            Ok(Box::new(IosGuard { id }))
        }
    }

    /// iOS の `FileWatch` 実装を返す（`device/file_watch.rs` の dispatch から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn FileWatch> {
        Arc::new(IosFileWatch)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;
