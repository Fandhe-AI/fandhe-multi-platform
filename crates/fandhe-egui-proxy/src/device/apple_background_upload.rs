//! macOS・iOS 共用のバックグラウンド送信実装（Issue #835 macOS、#836 iOS）。
//! `device/background_upload.rs::implementation` の macOS 経路と、`ios_background_upload`（#836）から呼ばれる。
//! `apple/fandhe_apple_background_upload.m`（background `NSURLSession`。`build.rs` の macOS・iOS 分岐が
//! コンパイル）を C ABI で呼ぶ薄いラッパー。`device/apple_background_download.rs` の送信版だが、
//! 「アプリ内に状態を持たない」点が異なる。
//!
//! # OS を正にする
//!
//! 送信中か・進み具合は [`BackgroundUpload::status`] のたびに `.m` の `getAllTasks`（`fandhe_apple_bgul_list`）
//! で読む。アプリ内に持つのは、OS の一覧から消える完了の結果 1 枠（[`Slot`]）だけで、OS の
//! `taskIdentifier` で照合する。`.m` の delegate は結果を [`fandhe_rust_bgul_completed`] で、読み直せの合図を
//! [`fandhe_rust_bgul_signal`] で返す。
//!
//! # 照合（契約表 2）
//!
//! 開始は `resume` の前に [`fandhe_rust_bgul_will_resume`] で識別子を渡し（開始から戻る前の完了も照合できる）、
//! 以後は識別子が一致する完了だけを採る。置き換え・取り消しの後に届いた古い転送の通知は捨てる。
//! このプロセスで開始していない（`expected` が無い）間は、前のプロセスから続く転送の完了として採る。
//! `taskIdentifier` が再起動の前後でぶつからないかは公式文書で確かめられていない。ぶつかるとしても、
//! 採るのは「開始前」か「同じ識別子」の完了だけで、開始後に別の識別子の結果を成功として採らない。
//!
//! # ロック順（契約表 11）
//!
//! [`Slot`] のロックを持ったまま `.m` を呼ばない。`.m` の待ち（`getAllTasks` の completion）は Rust のロックを
//! 取らない。`.m` は completion を期限なしで待つので、`start`・`cancel`・`status` は呼び出し側（ドライバ）が
//! 1 本の直列ワーカー（UI スレッドではない）から呼ぶ前提。`getAllTasks` の completion が来ない場合は守らない範囲。

// ホストの `cargo test`（Linux 等）では FFI 部分が存在せず純ロジックだけが使われる。
#![cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]

use std::sync::Mutex;

use fandhe_device::background_upload::{
    completion_status, OnSignal, UploadFailure, UploadStatus, FILE_ERROR,
};
use fandhe_device::DeviceError;

/// 一覧から読む task の上限件数。このアプリが作る転送は 1 つだけなので十分に大きい。
const LIST_CAP: usize = 8;

/// `NSURLSessionTaskState` の値（`.m` が生の値を渡す）。
const STATE_RUNNING: i32 = 0;
const STATE_SUSPENDED: i32 = 1;

/// OS の一覧の 1 件（`.m` の `FandheBgulTask` と同じ配置）。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TaskInfo {
    id: u64,
    state: i32,
    sent: u64,
    /// 総バイト数。不明なら負。
    expected: i64,
}

/// OS の一覧から消える完了の結果 1 枠と、今の転送の識別子。
#[derive(Debug, Default)]
struct Slot {
    /// このプロセスで開始した転送の `taskIdentifier`。`None` は未開始（前のプロセスから続く転送を引き受ける）。
    expected: Option<u64>,
    /// 照合済みの完了の結果。
    outcome: Option<UploadStatus>,
    /// 取り消しを依頼した後。次の `will_resume` までに届く完了は、取り消した要求のものとして捨てる。
    cancelled: bool,
}

impl Slot {
    const fn new() -> Self {
        Self {
            expected: None,
            outcome: None,
            cancelled: false,
        }
    }

    /// 取り消しが OS へ通った後に呼ぶ。完了前なら結果を `Cancelled` に確定し、以後の遅延した完了
    /// （成功を含む）は採らない（契約表 2）。完了済みの結果は上書きしない。
    fn cancel(&mut self) {
        self.cancelled = true;
        if self.expected.is_some() && self.outcome.is_none() {
            self.outcome = Some(UploadStatus::Cancelled);
        }
    }

    /// 前のプロセスから続く転送を `expected` に採用する（まだ何も照合していないときだけ）。
    /// 取り消しの前に呼び、`cancel` が `Cancelled` を確定できるようにする（契約表 3・10）。
    fn adopt(&mut self, tasks: &[TaskInfo]) {
        if self.expected.is_none() && self.outcome.is_none() {
            self.expected = tasks
                .iter()
                .find(|t| t.state == STATE_RUNNING || t.state == STATE_SUSPENDED)
                .map(|t| t.id);
        }
    }

    /// 開始する転送の識別子を受ける（`resume` の前）。前の結果は捨てる。
    fn will_resume(&mut self, id: u64) {
        self.expected = Some(id);
        self.outcome = None;
        self.cancelled = false;
    }

    /// 完了の通知を照合して記録する。置き換え後の古い転送の通知は捨てて `false`。
    /// 未知のコードを成功にしない。
    fn completed(
        &mut self,
        id: u64,
        code: i32,
        http_status: i32,
        sent: u64,
        redirected: bool,
    ) -> bool {
        if self.cancelled || self.expected.is_some_and(|e| e != id) {
            return false;
        }
        self.outcome = Some(match code {
            0 => completion_status(u16::try_from(http_status).ok(), sent, redirected),
            1 => UploadStatus::Cancelled,
            2 => UploadStatus::Failed(UploadFailure::Network),
            _ => UploadStatus::Failed(UploadFailure::Other),
        });
        true
    }
}

/// OS の一覧と結果の 1 枠から状態を決める純関数。
///
/// 1. 走っている転送（このプロセスの識別子、無ければ前のプロセスから続くもの）があれば、実行中は
///    `Sending`、保留は `Waiting`
/// 2. 無ければ照合済みの結果
/// 3. 結果も無いが開始済み（完了の通知の到着待ち）なら `Sending`。`Idle` や `Completed` にしない
/// 4. どれでもなければ `Idle`
fn status_from(tasks: &[TaskInfo], slot: &Slot) -> UploadStatus {
    let mine = |t: &&TaskInfo| slot.expected.is_none_or(|e| t.id == e);
    if let Some(t) = tasks
        .iter()
        .filter(mine)
        .find(|t| t.state == STATE_RUNNING || t.state == STATE_SUSPENDED)
    {
        return if t.state == STATE_SUSPENDED {
            UploadStatus::Waiting
        } else {
            UploadStatus::Sending {
                sent: t.sent,
                total: u64::try_from(t.expected).ok().filter(|n| *n > 0),
            }
        };
    }
    if let Some(outcome) = &slot.outcome {
        return outcome.clone();
    }
    if slot.expected.is_some() {
        let sent = tasks.iter().filter(mine).map(|t| t.sent).max().unwrap_or(0);
        return UploadStatus::Sending { sent, total: None };
    }
    UploadStatus::Idle
}

/// `.m` の開始コードを `DeviceError` へ写す（コードの写像の SSOT）。
fn start_error(code: i32) -> DeviceError {
    match code {
        1 => DeviceError::Unsupported("background upload"),
        2 => DeviceError::InvalidInput("background upload url"),
        3 => DeviceError::Internal(FILE_ERROR),
        _ => DeviceError::Internal("background upload"),
    }
}

static SLOT: Mutex<Slot> = Mutex::new(Slot::new());

/// OS の通知から呼ぶ合図の送り先（ドライバが登録する。後から登録したものが勝つ）。
static SIGNAL: Mutex<Option<OnSignal>> = Mutex::new(None);

fn slot() -> Result<std::sync::MutexGuard<'static, Slot>, DeviceError> {
    SLOT.lock()
        .map_err(|_| DeviceError::Internal("background upload"))
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use super::*;
    use fandhe_device::background_upload::{file_contents, BackgroundUpload};
    use fandhe_device::http::check_url;
    use std::ffi::{c_char, CString};
    use std::sync::Arc;

    // SAFETY: `apple/fandhe_apple_background_upload.m` が定義する C ABI（`build.rs` がコンパイル）。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_apple_bgul_init() -> i32;
        fn fandhe_apple_bgul_start(url: *const c_char, body: *const u8, len: usize) -> i32;
        fn fandhe_apple_bgul_cancel() -> i32;
        fn fandhe_apple_bgul_list(out: *mut TaskInfo, cap: usize, out_len: *mut usize) -> i32;
    }

    pub(super) struct AppleBackgroundUpload;

    /// OS の一覧を読む。`SLOT` のロックは持たずに呼ぶこと。
    #[allow(unsafe_code)]
    fn list_tasks() -> Result<Vec<TaskInfo>, DeviceError> {
        let mut tasks = [TaskInfo::default(); LIST_CAP];
        let mut len: usize = 0;
        // SAFETY: `tasks` は `LIST_CAP` 件の領域で、`.m` はそれを超えて書かない（`cap` で渡す）。
        // `len` は有効な書き込み先。`.m` は completion を待ち終えてから戻るので、戻った後に書かない。
        let code = unsafe { fandhe_apple_bgul_list(tasks.as_mut_ptr(), LIST_CAP, &mut len) };
        if code != 0 {
            return Err(DeviceError::Internal("background upload"));
        }
        Ok(tasks[..len.min(LIST_CAP)].to_vec())
    }

    /// session を作って実装を返す。作れなければ `None`（呼び出し側が `Unsupported` に倒す。
    /// ダミーの成功実装にはしない）。
    pub(super) fn init() -> Option<Arc<dyn BackgroundUpload>> {
        // SAFETY: 引数なし。`.m` 側は `dispatch_once` で 1 回だけ session を作る。
        #[allow(unsafe_code)]
        let code = unsafe { fandhe_apple_bgul_init() };
        (code == 0).then(|| Arc::new(AppleBackgroundUpload) as Arc<dyn BackgroundUpload>)
    }

    impl BackgroundUpload for AppleBackgroundUpload {
        #[allow(unsafe_code)]
        fn start(&self, url: &str) -> Result<(), DeviceError> {
            check_url(url)?;
            let c_url = CString::new(url)
                .map_err(|_| DeviceError::InvalidInput("background upload url"))?;
            let body = file_contents();
            // SAFETY: `c_url` と `body` は呼び出し中生きており、`.m` は同期でコピー（NSData / 文字列）して
            // ポインタを保持しない。`SLOT` のロックは持たない（`.m` が `will_resume` で取るため）。
            let code =
                unsafe { fandhe_apple_bgul_start(c_url.as_ptr(), body.as_ptr(), body.len()) };
            if code == 0 {
                Ok(())
            } else {
                Err(start_error(code))
            }
        }

        #[allow(unsafe_code)]
        fn cancel(&self) -> Result<(), DeviceError> {
            // 前のプロセスから続く転送を取り消すとき、識別子を先に採用して `Cancelled` を保てるようにする。
            let tasks = list_tasks()?;
            slot()?.adopt(&tasks);
            // SAFETY: 引数なし。`.m` は全 task への `[task cancel]` が終わるまで待つ。
            let code = unsafe { fandhe_apple_bgul_cancel() };
            match code {
                0 => {
                    slot()?.cancel();
                    Ok(())
                }
                1 => Err(DeviceError::Unsupported("background upload")),
                _ => Err(DeviceError::Internal("background upload")),
            }
        }

        fn status(&self) -> Result<UploadStatus, DeviceError> {
            let tasks = list_tasks()?;
            let guard = slot()?;
            Ok(status_from(&tasks, &guard))
        }

        fn set_signal(&self, on_signal: OnSignal) -> Result<(), DeviceError> {
            *SIGNAL
                .lock()
                .map_err(|_| DeviceError::Internal("background upload"))? = Some(on_signal);
            Ok(())
        }
    }

    /// 転送を再開する直前。`.m` が `resume` の前に呼ぶ。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_bgul_will_resume(task_id: u64) {
        if let Ok(mut s) = SLOT.lock() {
            s.will_resume(task_id);
        }
    }

    /// 転送の終了。`redirected` は開始 URL と異なる URL へ転送された（OS が追った）とき真。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_bgul_completed(
        task_id: u64,
        code: i32,
        http_status: i32,
        sent: u64,
        redirected: bool,
    ) {
        if let Ok(mut s) = SLOT.lock() {
            s.completed(task_id, code, http_status, sent, redirected);
        }
    }

    /// 「読み直せ」の合図。ロックの外で呼ぶ。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_bgul_signal() {
        let listener = SIGNAL.lock().ok().and_then(|s| s.clone());
        if let Some(listener) = listener {
            listener();
        }
        // iOS は入力が無いと egui が描画しない。OS コールバックの直後に次フレームを起こす（契約表 4）。
        #[cfg(target_os = "ios")]
        crate::ios::request_repaint();
    }
}

/// macOS・iOS の実装。session を作れなければ `None`。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn instance(
) -> Option<std::sync::Arc<dyn fandhe_device::background_upload::BackgroundUpload>> {
    use fandhe_device::background_upload::BackgroundUpload;
    use std::sync::{Arc, OnceLock};
    static INSTANCE: OnceLock<Option<Arc<dyn BackgroundUpload>>> = OnceLock::new();
    INSTANCE.get_or_init(imp::init).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::background_upload::FILE_SIZE;

    fn running(id: u64, sent: u64) -> TaskInfo {
        TaskInfo {
            id,
            state: STATE_RUNNING,
            sent,
            expected: FILE_SIZE as i64,
        }
    }

    #[test]
    fn stale_completion_after_replace_is_dropped() {
        let mut s = Slot::default();
        s.will_resume(2);
        assert!(!s.completed(1, 1, -1, 0, false));
        assert_eq!(s.outcome, None);
        assert!(s.completed(2, 0, 200, FILE_SIZE, false));
        assert_eq!(
            s.outcome,
            Some(UploadStatus::Completed {
                bytes: FILE_SIZE,
                http_status: 200
            })
        );
    }

    #[test]
    fn late_success_after_cancel_is_dropped() {
        let mut s = Slot::default();
        s.will_resume(1);
        s.cancel();
        assert!(!s.completed(1, 0, 200, FILE_SIZE, false));
        assert_eq!(s.outcome, Some(UploadStatus::Cancelled));
        // 次の開始で解除される。
        s.will_resume(2);
        assert!(s.completed(2, 0, 200, FILE_SIZE, false));
        assert!(matches!(s.outcome, Some(UploadStatus::Completed { .. })));
    }

    #[test]
    fn cancel_of_inherited_task_is_kept_as_cancelled() {
        let mut s = Slot::default();
        s.adopt(&[running(9, 5)]);
        s.cancel();
        // 後から届く取り消し完了・成功で上書きされず、一覧から消えても Idle に戻らない。
        assert!(!s.completed(9, 0, 200, FILE_SIZE, false));
        assert_eq!(status_from(&[], &s), UploadStatus::Cancelled);
    }

    #[test]
    fn adopt_without_task_keeps_idle() {
        let mut s = Slot::default();
        s.adopt(&[]);
        s.cancel();
        assert_eq!(status_from(&[], &s), UploadStatus::Idle);
    }

    #[test]
    fn cancel_keeps_finished_outcome() {
        let mut s = Slot::default();
        s.will_resume(1);
        s.completed(1, 0, 200, FILE_SIZE, false);
        s.cancel();
        assert!(matches!(s.outcome, Some(UploadStatus::Completed { .. })));
    }

    #[test]
    fn will_resume_clears_previous_outcome() {
        let mut s = Slot::default();
        s.will_resume(1);
        s.completed(1, 0, 200, FILE_SIZE, false);
        s.will_resume(2);
        assert_eq!(s.outcome, None);
    }

    #[test]
    fn completion_before_any_start_is_adopted() {
        let mut s = Slot::default();
        assert!(s.completed(41, 0, 200, FILE_SIZE, false));
        assert!(matches!(s.outcome, Some(UploadStatus::Completed { .. })));
    }

    #[test]
    fn codes_map_without_inventing_success() {
        let mut s = Slot::default();
        s.completed(1, 1, -1, 0, false);
        assert_eq!(s.outcome, Some(UploadStatus::Cancelled));
        s.completed(1, 2, -1, 0, false);
        assert_eq!(
            s.outcome,
            Some(UploadStatus::Failed(UploadFailure::Network))
        );
        s.completed(1, 99, 200, FILE_SIZE, false);
        assert_eq!(s.outcome, Some(UploadStatus::Failed(UploadFailure::Other)));
        // code 0 でも HTTP の状態が無い・サイズ違い・リダイレクトは成功にならない。
        s.completed(1, 0, -1, FILE_SIZE, false);
        assert_eq!(s.outcome, Some(UploadStatus::Failed(UploadFailure::Other)));
        s.completed(1, 0, 200, 10, false);
        assert!(matches!(
            s.outcome,
            Some(UploadStatus::Failed(UploadFailure::SizeMismatch { .. }))
        ));
        s.completed(1, 0, 200, FILE_SIZE, true);
        assert_eq!(
            s.outcome,
            Some(UploadStatus::Failed(UploadFailure::RedirectNotAllowed))
        );
    }

    #[test]
    fn running_and_suspended_tasks_map_to_sending_and_waiting() {
        let mut s = Slot::default();
        s.will_resume(5);
        assert_eq!(
            status_from(&[running(5, 100)], &s),
            UploadStatus::Sending {
                sent: 100,
                total: Some(FILE_SIZE)
            }
        );
        let suspended = TaskInfo {
            state: STATE_SUSPENDED,
            ..running(5, 0)
        };
        assert_eq!(status_from(&[suspended], &s), UploadStatus::Waiting);
    }

    #[test]
    fn unknown_total_is_none() {
        let s = Slot::default();
        let t = TaskInfo {
            expected: -1,
            ..running(1, 7)
        };
        assert_eq!(
            status_from(&[t], &s),
            UploadStatus::Sending {
                sent: 7,
                total: None
            }
        );
    }

    #[test]
    fn other_tasks_do_not_hide_my_outcome() {
        let mut s = Slot::default();
        s.will_resume(5);
        s.completed(5, 0, 200, FILE_SIZE, false);
        // 置き換え前の task（識別子 4）が一覧に残っていても、自分の結果を返す。
        assert!(matches!(
            status_from(&[running(4, 1)], &s),
            UploadStatus::Completed { .. }
        ));
    }

    #[test]
    fn missing_from_list_without_outcome_is_never_idle_or_completed() {
        let mut s = Slot::default();
        s.will_resume(5);
        assert!(matches!(status_from(&[], &s), UploadStatus::Sending { .. }));
        assert_eq!(status_from(&[], &Slot::default()), UploadStatus::Idle);
    }

    #[test]
    fn start_codes_map_to_errors() {
        assert!(matches!(start_error(1), DeviceError::Unsupported(_)));
        assert!(matches!(start_error(2), DeviceError::InvalidInput(_)));
        assert!(matches!(start_error(3), DeviceError::Internal(FILE_ERROR)));
        assert!(matches!(start_error(99), DeviceError::Internal(_)));
    }
}
