//! iOS 向けバックグラウンド処理実装（Issue #1045）。`BGTaskScheduler` +
//! `BGAppRefreshTaskRequest`（`ios/fandhe_ios_background_task.m`、`build.rs` が
//! feature `device` 有効時にコンパイル・リンクする）を `unsafe extern "C"` 3 関数
//! （[`ffi::register`]/[`ffi::submit`]/[`ffi::cancel`]）で呼ぶ。`unsafe` はこの
//! 呼び出しに閉じ `// SAFETY:` で契約を明記する（`macos_background_task.rs` と同型、
//! coding-rust.md の FFI 例外規定準拠）。
//!
//! `device/background_task.rs` の iOS 経路から [`IosBackgroundTask`] を使う
//! （`fandhe_device::background_task::BackgroundTask` 実装）。
//!
//! # 世代管理をしない（オーナー承認済み方針、2026-10-02）
//!
//! 予約の世代を厳密に追跡・照合することはしない。[`ScheduleState`] は OS への
//! 問い合わせ結果または自前の「予約した」印を出す参考値であり、発火
//! （[`on_fired`]）は現在の状態に関わらず必ず `Ran` にする。古い要求の発火が
//! 新しい予約の状態を `Ran` に変えることはあり得るが、参考値である以上
//! 許容する。確定した事実は [`BackgroundTask::last_run`] が返す記録のみで、
//! こちらは世代照合ではなく**時刻の新しい方を残す**規則で保護する
//! （[`on_fired`] 参照）。
//!
//! # 発火経路（新規プロセスが主経路）
//!
//! iOS のバックグラウンド発火は新規プロセス（アプリがバックグラウンドで再起動
//! された状態）で届くのが主経路で、そのプロセスのメモリ上の [`ScheduleState`] は
//! 常に `NotScheduled` から始まる。[`on_fired`] は現在の状態に関わらず必ず
//! 記録の保存を試みる（取り消した要求は OS が発火させないため、発火が届いた
//! 時点でそれは有効な実行とみなせる）。
//!
//! プロセスを再起動した直後は、OS に予約が残っていても `NotScheduled` を返す
//! （trait 契約で許容される、`getPendingTaskRequests` での復元は YAGNI）。

#[cfg(target_os = "ios")]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
#[cfg(target_os = "ios")]
use std::time::{SystemTime, UNIX_EPOCH};

use fandhe_device::background_task::{
    read_record, write_record_if_newer, BackgroundTask, RunRecord, RunResult, ScheduleState,
};
use fandhe_device::storage::Storage;
use fandhe_device::DeviceError;

use super::background_task::submit_code_to_result;

/// `submit` 時に渡す予約の最短開始猶予（秒）。iOS は実行時刻を保証しないため
/// 調整用の knob として定数にしておく（`ponytail:` iOS は実行時刻を保証しない。
/// lldb の擬似起動（対応表 `note` 参照）ではこの値は無関係）。
const EARLIEST_BEGIN_SECS: f64 = 60.0;

// 以下 `ffi_decl`/`ffi` は実 C ABI シンボルを参照するため `target_os = "ios"` のみで
// コンパイルする（host の `cargo test`（`cfg(test)` によるモジュール全体の
// コンパイル対象化）では存在しないシンボルのためリンクできない。モジュール doc
// 「モジュールの cfg」参照）。host テストは [`IosBackgroundTask::with_ffi`] へ
// モック関数ポインタを注入して検証する。
#[cfg(target_os = "ios")]
mod ffi_decl {
    // SAFETY: `ios/fandhe_ios_background_task.m`（`build.rs` が iOS ターゲットの
    // feature `device` 有効時にコンパイル・リンクする）が定義する C ABI。契約は
    // `.m` ファイル冒頭コメントに集約する。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        pub(super) fn fandhe_ios_background_task_register() -> i32;
        pub(super) fn fandhe_ios_background_task_submit(earliest_begin_secs: f64) -> i32;
        pub(super) fn fandhe_ios_background_task_cancel();
    }
}

#[cfg(target_os = "ios")]
mod ffi {
    /// `.m` 側の launch handler 登録を呼ぶ（[`super::register`] の実体）。
    #[allow(unsafe_code)]
    pub(super) fn register() -> bool {
        // SAFETY: 引数・戻り値ともプリミティブのみ。
        (unsafe { super::ffi_decl::fandhe_ios_background_task_register() }) == 0
    }

    /// `.m` 側の予約依頼を呼ぶ（`IosBackgroundTask::submit_fn` の既定実装。
    /// テストから差し替えられるよう関数ポインタとして保持される、モジュール doc 参照）。
    #[allow(unsafe_code)]
    pub(super) fn submit(earliest_begin_secs: f64) -> i32 {
        // SAFETY: 引数はプリミティブのみでポインタを経由しない。
        unsafe { super::ffi_decl::fandhe_ios_background_task_submit(earliest_begin_secs) }
    }

    /// `.m` 側の取り消しを呼ぶ（`IosBackgroundTask::cancel_fn` の既定実装）。
    #[allow(unsafe_code)]
    pub(super) fn cancel() {
        // SAFETY: 引数なし、戻り値なし。
        unsafe { super::ffi_decl::fandhe_ios_background_task_cancel() };
    }
}

/// `state` フィールドが保持する内部状態。
struct Inner {
    state: ScheduleState,
    /// 直近の `last_run` 保存が失敗したか（`true` の間は `last_run()` が `Err` を
    /// 返す。保存失敗を「未実行」へ畳まない、device-bridge-contracts.md
    /// 「6. 嘘の成功」参照）。
    write_failed: bool,
}

/// `BGTaskScheduler` による iOS 向け `BackgroundTask` 実装。
pub struct IosBackgroundTask {
    state: Mutex<Inner>,
    /// schedule/cancel の FFI 呼び出しを直列化する（`MacBackgroundTask::ffi` と
    /// 同じ理由。ロック順序は `ffi` → `state`、`on_fired` は `state` だけを取る）。
    ffi: Mutex<()>,
    storage: Arc<dyn Storage>,
    submit_fn: fn(f64) -> i32,
    cancel_fn: fn(),
}

impl IosBackgroundTask {
    /// 既定の FFI 実装で構築する（`device/background_task.rs::implementation` から
    /// 呼ばれる契約）。実 C ABI を参照するため iOS ターゲットのみ（モジュール doc 参照）。
    #[cfg(target_os = "ios")]
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self::with_ffi(storage, ffi::submit, ffi::cancel)
    }

    /// テストから任意の FFI 実装を注入する入口（`MacBackgroundTask::with_ffi` と
    /// 同じ設計）。host の `cargo test` から状態機械を検証できるよう常にコンパイルする。
    fn with_ffi(storage: Arc<dyn Storage>, submit_fn: fn(f64) -> i32, cancel_fn: fn()) -> Self {
        Self {
            state: Mutex::new(Inner {
                state: ScheduleState::NotScheduled,
                write_failed: false,
            }),
            ffi: Mutex::new(()),
            storage,
            submit_fn,
            cancel_fn,
        }
    }
}

impl BackgroundTask for IosBackgroundTask {
    fn schedule(&self) -> Result<(), DeviceError> {
        let _ffi_guard = self.ffi.lock().expect("lock は成功する");
        let code = (self.submit_fn)(EARLIEST_BEGIN_SECS);
        let result = submit_code_to_result(code);

        let mut inner = self.state.lock().expect("lock は成功する");
        // `.m` 側の `submit` は新規要求を送信する前に既存要求を必ず取り消す
        // （`fandhe_ios_background_task.m` 冒頭コメント「置き換え契約」参照）。
        // そのため送信が失敗した時点で OS 側に予約は残っていない。失敗時も
        // `Scheduled` を維持すると、UI は「予約済み」と表示するが OS 側の予約は
        // 実在しない状態不整合になる（嘘の成功、device-bridge-contracts.md
        // 「9. 失敗時のリソース解放・共有状態復元」「6. 嘘の成功」、codex P1 #1050）。
        inner.state = if result.is_ok() {
            // 二重予約は置き換え契約。OS 側は常に高々 1 件のため、Rust 側の
            // 状態もそのまま `Scheduled` に更新する（世代を使わない理由はモジュール
            // doc 参照）。
            ScheduleState::Scheduled
        } else {
            ScheduleState::NotScheduled
        };
        result
    }

    fn cancel(&self) -> Result<(), DeviceError> {
        let _ffi_guard = self.ffi.lock().expect("lock は成功する");
        (self.cancel_fn)();
        self.state.lock().expect("lock は成功する").state = ScheduleState::NotScheduled;
        // 未予約での取り消しもべき等に `Ok`（モジュール doc 参照）。
        Ok(())
    }

    fn state(&self) -> Result<ScheduleState, DeviceError> {
        Ok(self.state.lock().expect("lock は成功する").state)
    }

    fn last_run(&self) -> Result<Option<RunRecord>, DeviceError> {
        if self.state.lock().expect("lock は成功する").write_failed {
            // 保存失敗を「未実行」に見せない（device-bridge-contracts.md「6. 嘘の成功」）。
            return Err(DeviceError::Internal("background task record write"));
        }
        read_record(self.storage.as_ref())
    }
}

/// [`on_fired`] の記録の比較〜書き込みを直列化するロック（プロセス内に
/// インスタンスは 1 つのため static で足りる）。
static RECORD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 発火時の保存と状態更新本体（[`fandhe_rust_ios_background_task_fired`] から
/// 呼ばれる、テストから直接呼べるよう分離）。`ffi` ロックには触れない
/// （モジュール doc「世代管理をしない」参照）。
///
/// 通常完了経路と期限切れ経路（`.m` 側）は別キューから並行して呼ばれうる
/// （`.m` ファイル冒頭コメント参照）。**記録は時刻の新しい方を残す**規則で
/// この競合を解く: 既存の記録より新しい（`ran_at_unix_ms` が既存以上）場合だけ
/// 上書きする。古い方が後から届いても無視し、`Ok` として扱う（嘘の成功ではない。
/// 既存の正しい記録を保つ選択であり、disk I/O 自体は成功している）。
///
/// disk I/O（[`read_record`]/[`write_record_if_newer`]）は `state` ロックの外で行う。
/// ロックを握ったまま保存を待つと、同じロックを取る対向経路（期限切れ通知等）が
/// 保存完了まで止まる（codex P1 指摘、#1050）。ロックは保存結果を状態へ反映する
/// 一瞬だけ取る。
///
/// 戻り値は保存の成否（`.m` 側の `setTaskCompletedWithSuccess:` 判定に使う）。
fn on_fired(task: &IosBackgroundTask, result: RunResult, now_unix_ms: u64) -> bool {
    let record = RunRecord {
        ran_at_unix_ms: now_unix_ms,
        result,
    };
    // 比較から書き込みまでを記録専用のロックで直列化する（通常完了と期限切れが
    // 同じ旧記録を読んで、古い方が後から上書きするのを防ぐ。codex・Bugbot 指摘、
    // #1050）。`state` ロックとは別にし、保存中に `state` を握らない設計は保つ。
    // 状態（`write_failed` 等）の反映もこのロックの内側で行い、保存の順序と
    // 失敗状態の反映順をそろえる（並行する発火で後の保存失敗が先の成功で上書き
    // されないように。PR #1050 codex 指摘）。ロック順は `RECORD_LOCK` → `state`
    // の一方向のみ（`state` を握ったまま `RECORD_LOCK` を取る経路は無い）。
    let _record_guard = RECORD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 既存記録より古ければ書かない・読み出し I/O エラーなら書かずに `Err`・壊れた記録は
    // 上書きして回復する、の規則は `write_record_if_newer` に一本化する（PR #1050 codex
    // 指摘。自前の比較は I/O エラーでも上書きしていた）。
    let write_result = write_record_if_newer(task.storage.as_ref(), &record);

    let mut inner = task.state.lock().expect("lock は成功する");
    match write_result {
        Ok(()) => {
            inner.write_failed = false;
            // 世代照合はしない（参考値、モジュール doc 参照）。
            inner.state = ScheduleState::Ran;
            true
        }
        Err(_) => {
            inner.write_failed = true;
            inner.state = ScheduleState::NotScheduled;
            false
        }
    }
}

/// プロセス static（`device/background_task.rs::implementation` が構築した
/// 唯一のインスタンスを保持する。発火コールバックは FFI 境界を越えて届くため
/// `Arc` 経由でしか参照できない、`macos_background_task.rs::INSTANCE` と同じ必要性）。
#[cfg(target_os = "ios")]
static INSTANCE: OnceLock<Arc<IosBackgroundTask>> = OnceLock::new();

/// `device/background_task.rs::implementation` から 1 度だけ構築する
/// （`Documents/` を保存先とする `FsStorage`、`ios::documents_dir()` が失敗したら
/// `None`。画面を開かないままバックグラウンド起動されても保存先を作れるよう、
/// 発火コールバックからもこの関数を経由する）。
#[cfg(target_os = "ios")]
pub(crate) fn instance() -> Option<Arc<IosBackgroundTask>> {
    use fandhe_device::storage::FsStorage;

    static STORAGE_FAILED: OnceLock<()> = OnceLock::new();
    if STORAGE_FAILED.get().is_some() {
        return None;
    }
    match INSTANCE.get() {
        Some(task) => Some(task.clone()),
        None => match super::ios::documents_dir() {
            Ok(dir) => {
                let task = Arc::new(IosBackgroundTask::new(Arc::new(FsStorage::new(dir))));
                Some(INSTANCE.get_or_init(|| task).clone())
            }
            Err(_) => {
                let _ = STORAGE_FAILED.set(());
                None
            }
        },
    }
}

/// `.m` 側の `registerForTaskWithIdentifier:` を main thread から起動完了前に呼ぶ
/// 安全なラッパー（`main.rs` の iOS 向け `fn main` から `ios::run()` の前に 1 回だけ
/// 呼ばれる契約、実装計画 §3.5）。
#[cfg(target_os = "ios")]
pub fn register() {
    // SAFETY: 引数なし、戻り値はプリミティブのみ。呼び出しのタイミング契約
    // （起動完了前・main thread）は呼び出し元（`main.rs`）が守る。
    ffi::register();
}

/// `.m` 側 launch handler から呼ばれる `extern "C"` エクスポート関数（`.m` ファイル
/// 冒頭コメント参照）。`result`: 0 = Completed、1 = Expired。戻り値: 0 = 保存成功、
/// 1 = 失敗（[`instance`] が `None`、または未知の `result` 値も失敗として扱う。
/// `.m` 側はこれを `setTaskCompletedWithSuccess:` の判定に使う）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_ios_background_task_fired(result: i32) -> i32 {
    let Some(task) = instance() else {
        return 1;
    };
    let run_result = match result {
        0 => RunResult::Completed,
        1 => RunResult::Expired,
        _ => return 1,
    };
    let now_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if on_fired(&task, run_result, now_unix_ms) {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct MemStorage(StdMutex<Option<Vec<u8>>>);

    impl Storage for MemStorage {
        fn read(&self, _key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
            Ok(self.0.lock().expect("lock は成功する").clone())
        }

        fn write(&self, _key: &str, bytes: &[u8]) -> Result<(), DeviceError> {
            *self.0.lock().expect("lock は成功する") = Some(bytes.to_vec());
            Ok(())
        }
    }

    struct FailingWriteStorage;

    impl Storage for FailingWriteStorage {
        fn read(&self, _key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
            Ok(None)
        }

        fn write(&self, _key: &str, _bytes: &[u8]) -> Result<(), DeviceError> {
            Err(DeviceError::Internal("write failure"))
        }
    }

    // `submit_fn`/`cancel_fn` は固定コードを返すだけの fn ポインタにする（クロージャは
    // 関数ポインタ型に変換できず、共有 static な可変状態は `cargo test` の並列実行で
    // 他テストと競合するため使わない。`macos_background_task.rs` の `always_ok`/
    // `always_fail` と同じ発想だが、本ファイルは状態を持つ静的変数自体を避ける）。
    fn always_ok(_earliest_begin_secs: f64) -> i32 {
        0
    }
    fn always_fail_disabled(_earliest_begin_secs: f64) -> i32 {
        1
    }
    fn always_fail_permission_denied(_earliest_begin_secs: f64) -> i32 {
        2
    }
    fn always_fail_unsupported(_earliest_begin_secs: f64) -> i32 {
        3
    }
    fn always_fail_internal(_earliest_begin_secs: f64) -> i32 {
        4
    }
    fn noop_cancel() {}

    fn task_with(submit_fn: fn(f64) -> i32, storage: Arc<dyn Storage>) -> IosBackgroundTask {
        IosBackgroundTask::with_ffi(storage, submit_fn, noop_cancel)
    }

    #[test]
    fn schedule_success_sets_scheduled() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        assert!(task.schedule().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::Scheduled);
    }

    #[test]
    fn schedule_failure_disabled_keeps_not_scheduled() {
        let task = task_with(always_fail_disabled, Arc::new(MemStorage::default()));
        assert!(matches!(
            task.schedule(),
            Err(DeviceError::Disabled("background task"))
        ));
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
    }

    #[test]
    fn schedule_failure_permission_denied_keeps_not_scheduled() {
        let task = task_with(
            always_fail_permission_denied,
            Arc::new(MemStorage::default()),
        );
        assert!(matches!(
            task.schedule(),
            Err(DeviceError::PermissionDenied("background task"))
        ));
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
    }

    #[test]
    fn schedule_failure_unsupported_keeps_not_scheduled() {
        let task = task_with(always_fail_unsupported, Arc::new(MemStorage::default()));
        assert!(matches!(
            task.schedule(),
            Err(DeviceError::Unsupported("background task"))
        ));
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
    }

    #[test]
    fn schedule_failure_unknown_code_maps_to_internal_and_keeps_not_scheduled() {
        let task = task_with(always_fail_internal, Arc::new(MemStorage::default()));
        assert!(matches!(
            task.schedule(),
            Err(DeviceError::Internal("background task schedule"))
        ));
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
    }

    #[test]
    fn double_schedule_replaces_and_stays_scheduled() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        assert!(task.schedule().is_ok());
        assert!(task.schedule().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::Scheduled);
    }

    // codex P1 / Cursor Bugbot 指摘（#1050）の再現: `.m` 側の `submit` は新規送信前に
    // 既存要求を必ず取り消すため（ファイル冒頭コメント参照）、一度 `Scheduled` に
    // なった後の再 submit が失敗すると OS 側には予約が残らない。Rust 側の状態も
    // `NotScheduled` へ追随することを確認する（嘘の成功の回帰防止）。
    #[test]
    fn reschedule_failure_after_success_resets_to_not_scheduled() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        assert!(task.schedule().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::Scheduled);

        let task = IosBackgroundTask {
            submit_fn: always_fail_disabled,
            ..task
        };
        assert!(task.schedule().is_err());
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
    }

    #[test]
    fn cancel_without_prior_schedule_is_ok_and_calls_ffi() {
        static DEDICATED_CANCEL_CALLS: AtomicI32 = AtomicI32::new(0);
        fn dedicated_cancel() {
            DEDICATED_CANCEL_CALLS.fetch_add(1, Ordering::SeqCst);
        }

        let task = IosBackgroundTask::with_ffi(
            Arc::new(MemStorage::default()),
            always_ok,
            dedicated_cancel,
        );
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
        assert!(task.cancel().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
        assert_eq!(DEDICATED_CANCEL_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancel_after_schedule_resets_to_not_scheduled() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        task.schedule().unwrap();
        assert!(task.cancel().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
    }

    #[test]
    fn fired_while_scheduled_sets_ran_and_saves_record() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        task.schedule().unwrap();

        assert!(on_fired(&task, RunResult::Completed, 42_000));
        assert_eq!(task.state().unwrap(), ScheduleState::Ran);
        let record = task.last_run().unwrap().expect("記録が保存されている");
        assert_eq!(record.ran_at_unix_ms, 42_000);
        assert_eq!(record.result, RunResult::Completed);
    }

    #[test]
    fn fired_while_not_scheduled_still_saves_record_and_sets_ran() {
        // 新規プロセスでのバックグラウンド起動（モジュール doc「発火経路」参照）:
        // メモリ上の状態は NotScheduled から始まるが、発火自体は正当な実行のため
        // 記録は保存する。状態は世代照合をしない参考値のため `Ran` にする
        // （モジュール doc「世代管理をしない」参照）。
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);

        assert!(on_fired(&task, RunResult::Completed, 1_000));
        assert_eq!(task.state().unwrap(), ScheduleState::Ran);
        let record = task.last_run().unwrap().expect("記録が保存されている");
        assert_eq!(record.ran_at_unix_ms, 1_000);
    }

    #[test]
    fn fired_keeps_newer_existing_record_when_later_fired_is_older() {
        // 通常完了経路と期限切れ経路が並行して届き、時刻の古い方が後から
        // 書き込まれようとした場合でも、既存の新しい記録を残す（モジュール doc
        // 「記録は時刻の新しい方を残す」規則参照）。
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        task.schedule().unwrap();

        assert!(on_fired(&task, RunResult::Expired, 10_000));
        assert!(on_fired(&task, RunResult::Completed, 5_000));

        let record = task.last_run().unwrap().expect("記録が保存されている");
        assert_eq!(record.ran_at_unix_ms, 10_000);
        assert_eq!(record.result, RunResult::Expired);
    }

    #[test]
    fn fired_expired_is_saved_as_expired() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        task.schedule().unwrap();

        assert!(on_fired(&task, RunResult::Expired, 7_000));
        let record = task.last_run().unwrap().expect("記録が保存されている");
        assert_eq!(record.result, RunResult::Expired);
    }

    #[test]
    fn fired_write_failure_sets_last_run_err_not_never_ran() {
        let task = task_with(always_ok, Arc::new(FailingWriteStorage));
        task.schedule().unwrap();

        assert!(!on_fired(&task, RunResult::Completed, 1_000));
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
        // 書き込み失敗は「未実行」(`Ok(None)`) に化けず `Err` になる
        // （device-bridge-contracts.md「6. 嘘の成功」）。
        assert!(task.last_run().is_err());
    }

    #[test]
    fn write_failed_persists_until_next_successful_run() {
        let task = task_with(always_ok, Arc::new(FailingWriteStorage));
        task.schedule().unwrap();
        on_fired(&task, RunResult::Completed, 1);
        assert!(task.last_run().is_err());
        task.cancel().unwrap();
        assert!(task.last_run().is_err());
    }
}
