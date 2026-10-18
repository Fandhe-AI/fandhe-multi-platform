//! macOS・iOS 共用のバックグラウンドダウンロード実装（Issue #390 macOS、iOS は b-ios #391）。
//! `device/background_download.rs::implementation` の macOS 経路から呼ばれる。
//! `apple/fandhe_apple_background_download.m`（background `NSURLSession`。`build.rs` の macOS・iOS 分岐が
//! コンパイル）を C ABI で呼ぶ薄いラッパーで、転送の世代と状態は本モジュールの `Mutex<Inner>` が持つ。
//! `.m` の delegate（直列キュー）が [`fandhe_rust_bgdl_progress`] 等を呼び、Rust 側が世代を照合して
//! 状態を更新する。状態更新の判定は [`Inner`] の純ロジックに切り出し、ObjC なしでテストする。
//! コード → 結果の写像（[`finish_status`]・[`start_error`]）も本モジュールが SSOT で、`.m` では写さない。
//!
//! # 世代（契約表 2）
//!
//! 世代は開始前に Rust で採番して `.m` に渡し、task の `taskDescription` に入れる。開始の呼び出しから
//! 戻る前に届いたコールバックも照合できる。判定と更新は同じロックの内側で行い、取り消し・置き換え
//! 後に届いた古い通知は捨てる。ロックを持ったまま `.m` を呼ばない（デッドロック回避、契約表 11）。
//!
//! # 再起動後の再接続
//!
//! 起動時に `.m` が同じ identifier の session から走行中の task を拾い、[`Inner::reconnect`] で
//! `Running` に戻す。まだ `start`/`cancel` していない場合だけ反映する（後から来た再接続通知で、
//! 利用者の操作を巻き戻さない）。背景で完了・失敗した結果は再起動で失われ `Idle` になる（守らない範囲）。

// ホストの `cargo test`（Linux 等）では FFI 部分が存在せず純ロジックだけが使われる。
#![cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]

use std::sync::Mutex;

use fandhe_device::background_download::{completion_status, DownloadStatus};
use fandhe_device::http::check_url;
use fandhe_device::DeviceError;

/// 失敗の固定文言（URL・パス・OS の生の文言を含めない、A09）。
const FAILED_REDIRECT: &str = "リダイレクト先が許可されていません";
const FAILED_NETWORK: &str = "接続できませんでした";
const FAILED_SAVE: &str = "ファイルを保存できませんでした";
const FAILED_OTHER: &str = "ダウンロードに失敗しました";
const FAILED_START: &str = "開始できませんでした";

/// 転送の共有状態。
#[derive(Debug)]
struct Inner {
    next_gen: u64,
    /// 有効な転送の世代。`None` は取り消し済み・終了済み・未開始。
    current: Option<u64>,
    /// `start`/`cancel` を一度でも受けたか（再接続通知が操作を巻き戻さないための印）。
    touched: bool,
    status: DownloadStatus,
}

impl Inner {
    const fn new() -> Self {
        Self {
            next_gen: 0,
            current: None,
            touched: false,
            status: DownloadStatus::Idle,
        }
    }

    /// 世代の初期値を起動時刻（ミリ秒）にする。OS の task の世代表記は再起動後も残るため、
    /// 前プロセスの世代と衝突しない採番にする。採番済みなら何もしない（時刻は種であり経過時間の計測ではない）。
    fn seed_once(&mut self, now_ms: u64) {
        if self.next_gen == 0 {
            self.next_gen = now_ms;
        }
    }

    /// 新しい転送を始める（置き換え）。世代を返す。
    fn begin(&mut self) -> u64 {
        self.next_gen += 1;
        self.current = Some(self.next_gen);
        self.touched = true;
        self.status = DownloadStatus::Running {
            received: 0,
            total: None,
        };
        self.next_gen
    }

    /// 利用者の取り消し。以後の古い通知は世代不一致で捨てられる。
    fn cancel(&mut self) {
        self.current = None;
        self.touched = true;
        self.status = DownloadStatus::Cancelled;
    }

    /// `.m` への開始依頼が失敗した。現在の世代のときだけ確定する。
    fn start_failed(&mut self, gen: u64, status: DownloadStatus) {
        if self.current == Some(gen) {
            self.current = None;
            self.status = status;
        }
    }

    fn progress(&mut self, gen: u64, received: u64, total: i64) {
        if self.current == Some(gen) {
            self.status = DownloadStatus::Running {
                received,
                total: u64::try_from(total).ok().filter(|t| *t > 0),
            };
        }
    }

    /// 一時ファイルを保存先へ移してよいか。HTTP の状態・サイズ・最終 URL を確かめ、
    /// 拒否したらここで `Failed` を確定する（`.m` は一時ファイルを捨てる）。
    fn accept(&mut self, gen: u64, http_status: i32, bytes: u64, final_url: &str) -> bool {
        if self.current != Some(gen) {
            return false;
        }
        let verdict = if check_url(final_url).is_err() {
            DownloadStatus::Failed(FAILED_REDIRECT.to_string())
        } else {
            completion_status(u16::try_from(http_status).ok(), bytes)
        };
        match verdict {
            DownloadStatus::Completed { .. } => true,
            failed => {
                self.current = None;
                self.status = failed;
                false
            }
        }
    }

    /// 転送の終了通知。現在の世代のときだけ反映し、反映したら `true`。
    fn finish(&mut self, gen: u64, code: i32, bytes: u64) -> bool {
        if self.current != Some(gen) {
            return false;
        }
        self.current = None;
        self.status = finish_status(code, bytes);
        true
    }

    /// 起動時の再接続。操作前で何も追っていないときだけ `Running` に戻す。
    fn reconnect(&mut self, gen: u64) {
        self.next_gen = self.next_gen.max(gen);
        if !self.touched && self.current.is_none() {
            self.current = Some(gen);
            self.status = DownloadStatus::Running {
                received: 0,
                total: None,
            };
        }
    }
}

/// `.m` の終了コードを状態へ写す純関数。0 = 完了、1 = 取り消し、2 = 接続系、3 = 保存失敗、他 = 失敗。
/// 未知のコードは成功にしない。
fn finish_status(code: i32, bytes: u64) -> DownloadStatus {
    match code {
        0 => completion_status(None, bytes),
        1 => DownloadStatus::Cancelled,
        2 => DownloadStatus::Failed(FAILED_NETWORK.to_string()),
        3 => DownloadStatus::Failed(FAILED_SAVE.to_string()),
        _ => DownloadStatus::Failed(FAILED_OTHER.to_string()),
    }
}

/// `.m` の開始コードを契約上のエラーへ写す純関数。1 = session なし、2 = URL 不正、他 = 内部。
fn start_error(code: i32) -> DeviceError {
    match code {
        1 => DeviceError::Unsupported("background download"),
        2 => DeviceError::InvalidInput("background download url"),
        _ => DeviceError::Internal("background download"),
    }
}

static SHARED: Mutex<Inner> = Mutex::new(Inner::new());

fn lock() -> Result<std::sync::MutexGuard<'static, Inner>, DeviceError> {
    SHARED
        .lock()
        .map_err(|_| DeviceError::Internal("background download"))
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use super::*;
    use fandhe_device::background_download::BackgroundDownload;
    use std::ffi::{c_char, CStr, CString};
    use std::sync::Arc;

    // SAFETY: `apple/fandhe_apple_background_download.m` が定義する C ABI（`build.rs` がコンパイル）。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_apple_bgdl_init() -> i32;
        fn fandhe_apple_bgdl_start(url: *const c_char, gen: u64) -> i32;
        fn fandhe_apple_bgdl_cancel(upto_gen: u64) -> i32;
    }

    pub(super) struct AppleBackgroundDownload;

    /// session を作って再接続を始め、実装を返す。session を作れなければ `None`（呼び出し側が
    /// `Unsupported` に倒す。ダミーの成功実装にはしない）。
    #[cfg_attr(target_os = "ios", allow(dead_code))]
    pub(super) fn init() -> Option<Arc<dyn BackgroundDownload>> {
        // SAFETY: 引数なし。`.m` 側は `dispatch_once` で 1 回だけ session を作る。
        #[allow(unsafe_code)]
        let code = unsafe { fandhe_apple_bgdl_init() };
        (code == 0).then(|| Arc::new(AppleBackgroundDownload) as Arc<dyn BackgroundDownload>)
    }

    impl BackgroundDownload for AppleBackgroundDownload {
        #[allow(unsafe_code)]
        fn start(&self, url: &str) -> Result<(), DeviceError> {
            check_url(url)?;
            let c_url = CString::new(url)
                .map_err(|_| DeviceError::InvalidInput("background download url"))?;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_millis() as u64);
            let gen = {
                let mut g = lock()?;
                g.seed_once(now_ms);
                g.begin()
            };
            // SAFETY: `c_url` は呼び出し中生きており、`.m` は同期でコピーしてポインタを保持しない。
            // ロックは外してから呼ぶ（delegate が同じロックを取るため）。
            let code = unsafe { fandhe_apple_bgdl_start(c_url.as_ptr(), gen) };
            if code == 0 {
                return Ok(());
            }
            let err = start_error(code);
            let status = if matches!(err, DeviceError::Unsupported(_)) {
                DownloadStatus::Idle
            } else {
                DownloadStatus::Failed(FAILED_START.to_string())
            };
            lock()?.start_failed(gen, status);
            Err(err)
        }

        #[allow(unsafe_code)]
        fn cancel(&self) -> Result<(), DeviceError> {
            // 取り消し対象は呼び出し時点までに採番した世代。直後の `start` の新 task は巻き込まない。
            let upto = {
                let mut g = lock()?;
                g.cancel();
                g.next_gen
            };
            // SAFETY: 値渡しのみ。`.m` は世代 `upto` 以下の task を取り消し、保存先のファイルを消す（冪等）。
            unsafe { fandhe_apple_bgdl_cancel(upto) };
            Ok(())
        }

        fn status(&self) -> Result<DownloadStatus, DeviceError> {
            Ok(lock()?.status.clone())
        }
    }

    /// 受信進捗。`total` が負なら総バイト数は不明。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_bgdl_progress(gen: u64, received: u64, total: i64) {
        if let Ok(mut s) = SHARED.lock() {
            s.progress(gen, received, total);
        }
        repaint();
    }

    /// 一時ファイルを保存先へ移してよいかの判定（delegate 内で同期に呼ばれる）。
    ///
    /// # Safety
    ///
    /// `final_url` は NUL 終端の C 文字列、または null。
    #[allow(unsafe_code)]
    #[no_mangle]
    unsafe extern "C" fn fandhe_rust_bgdl_accept(
        gen: u64,
        http_status: i32,
        bytes: u64,
        final_url: *const c_char,
    ) -> bool {
        let url = if final_url.is_null() {
            String::new()
        } else {
            // SAFETY: 呼び出し元（`.m`）が有効な NUL 終端文字列を渡す契約。この呼び出しの間だけ借りる。
            unsafe { CStr::from_ptr(final_url) }
                .to_string_lossy()
                .into_owned()
        };
        let ok = SHARED
            .lock()
            .map(|mut s| s.accept(gen, http_status, bytes, &url))
            .unwrap_or(false);
        repaint();
        ok
    }

    /// 転送の終了。反映したら `true`（`code == 0` で `false` なら、`.m` は移した保存先を消す）。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_bgdl_finished(gen: u64, code: i32, bytes: u64) -> bool {
        let adopted = SHARED
            .lock()
            .map(|mut s| s.finish(gen, code, bytes))
            .unwrap_or(false);
        repaint();
        adopted
    }

    /// 起動時の再接続（走行中の task が見つかった）。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_bgdl_reconnected(gen: u64) {
        if let Ok(mut s) = SHARED.lock() {
            s.reconnect(gen);
        }
        repaint();
    }

    /// iOS では OS コールバックが状態を変えた直後に次フレームを起こす（契約表 4。入力が無いと
    /// egui が描画せず表示が止まる）。macOS は画面側のポーリング予約で足りるので何もしない。
    fn repaint() {
        #[cfg(target_os = "ios")]
        crate::ios::request_repaint();
    }
}

/// macOS の実装。session を作れなければ `None`。
#[cfg(target_os = "macos")]
pub(crate) fn instance(
) -> Option<std::sync::Arc<dyn fandhe_device::background_download::BackgroundDownload>> {
    use fandhe_device::background_download::BackgroundDownload;
    use std::sync::{Arc, OnceLock};
    static INSTANCE: OnceLock<Option<Arc<dyn BackgroundDownload>>> = OnceLock::new();
    INSTANCE.get_or_init(imp::init).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(inner: &mut Inner) -> u64 {
        inner.begin()
    }

    #[test]
    fn stale_generation_notifications_are_dropped() {
        let mut i = Inner::new();
        let old = running(&mut i);
        let new = running(&mut i); // 置き換え
        i.progress(old, 10, 40);
        assert!(!i.accept(old, 200, 40, "http://127.0.0.1/x"));
        assert!(!i.finish(old, 0, 40));
        assert_eq!(
            i.status,
            DownloadStatus::Running {
                received: 0,
                total: None
            }
        );
        i.progress(new, 10, 40);
        assert_eq!(
            i.status,
            DownloadStatus::Running {
                received: 10,
                total: Some(40)
            }
        );
    }

    #[test]
    fn completion_after_cancel_is_not_success() {
        let mut i = Inner::new();
        let gen = running(&mut i);
        i.cancel();
        assert!(!i.accept(gen, 200, 40, "http://127.0.0.1/x"));
        assert!(!i.finish(gen, 0, 40));
        assert_eq!(i.status, DownloadStatus::Cancelled);
    }

    #[test]
    fn accept_rejects_http_error_empty_and_redirect_to_public_http() {
        for (http, bytes, url) in [
            (404, 40, "http://127.0.0.1/x"),
            (200, 0, "http://127.0.0.1/x"),
            (200, 40, "http://example.com/x"),
            (200, 40, ""),
        ] {
            let mut i = Inner::new();
            let gen = running(&mut i);
            assert!(!i.accept(gen, http, bytes, url), "{http} {bytes} {url}");
            assert!(matches!(i.status, DownloadStatus::Failed(_)));
            // 拒否後に遅れて届く完了で成功にならない。
            assert!(!i.finish(gen, 0, 40));
            assert!(matches!(i.status, DownloadStatus::Failed(_)));
        }
    }

    #[test]
    fn accept_then_finish_completes() {
        let mut i = Inner::new();
        let gen = running(&mut i);
        assert!(i.accept(gen, 200, 40, "http://127.0.0.1:8787/items.json"));
        assert!(i.finish(gen, 0, 40));
        assert_eq!(i.status, DownloadStatus::Completed { bytes: 40 });
    }

    #[test]
    fn finish_codes_map_cancel_failure_and_unknown() {
        assert_eq!(finish_status(1, 0), DownloadStatus::Cancelled);
        for code in [2, 3, 4, 99, -1] {
            assert!(matches!(finish_status(code, 0), DownloadStatus::Failed(_)));
        }
        assert!(matches!(finish_status(0, 0), DownloadStatus::Failed(_)));
        assert_eq!(finish_status(0, 5), DownloadStatus::Completed { bytes: 5 });
    }

    #[test]
    fn start_codes_map_to_contract_errors() {
        assert!(matches!(start_error(1), DeviceError::Unsupported(_)));
        assert!(matches!(start_error(2), DeviceError::InvalidInput(_)));
        assert!(matches!(start_error(7), DeviceError::Internal(_)));
    }

    #[test]
    fn start_failure_only_settles_current_generation() {
        let mut i = Inner::new();
        let old = running(&mut i);
        let new = running(&mut i);
        i.start_failed(old, DownloadStatus::Failed("x".into()));
        assert!(matches!(i.status, DownloadStatus::Running { .. }));
        i.start_failed(new, DownloadStatus::Failed("x".into()));
        assert!(matches!(i.status, DownloadStatus::Failed(_)));
    }

    #[test]
    fn reconnect_keeps_next_gen_and_does_not_undo_user_actions() {
        let mut i = Inner::new();
        i.reconnect(5);
        assert_eq!(i.current, Some(5));
        assert!(matches!(i.status, DownloadStatus::Running { .. }));
        assert_eq!(i.begin(), 6);

        let mut j = Inner::new();
        j.cancel();
        j.reconnect(9);
        assert_eq!(j.current, None);
        assert_eq!(j.status, DownloadStatus::Cancelled);
        assert_eq!(j.begin(), 10);
    }

    #[test]
    fn seed_once_avoids_previous_process_generations() {
        let mut i = Inner::new();
        i.seed_once(1_000);
        assert_eq!(i.begin(), 1_001);
        i.seed_once(5); // 採番済みなら無視
        assert_eq!(i.begin(), 1_002);
    }

    #[test]
    fn unknown_total_is_none() {
        let mut i = Inner::new();
        let gen = running(&mut i);
        i.progress(gen, 3, -1);
        assert_eq!(
            i.status,
            DownloadStatus::Running {
                received: 3,
                total: None
            }
        );
    }
}
