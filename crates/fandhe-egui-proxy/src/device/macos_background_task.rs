//! macOS 向けバックグラウンド処理実装（Issue #1043）。`NSBackgroundActivityScheduler`
//! （`macos/fandhe_macos_background_task.m`、`build.rs` が macOS ターゲットの feature
//! `device` 有効時にコンパイル・リンクする）を `unsafe extern "C"` 2 関数（[`ffi::schedule`]/
//! [`ffi::cancel`]）で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する
//! （`apple_mic_record.rs` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! `device/background_task.rs` の macOS 経路から [`MacBackgroundTask`] を使う
//! （`fandhe_device::background_task::BackgroundTask` 実装）。
//!
//! # 世代管理（`device-bridge-contracts.md`「2. 並行性・世代」）
//!
//! `schedule`/`cancel` のたびに `generation` を進める。`.m` 側の block（発火時）が
//! 渡す `generation` が現在の世代と一致し、かつ `ScheduleState::Scheduled` のときだけ
//! 実行結果を保存する。取り消し後・置き換え後に古い世代の発火が届いても捨てる
//! （二重予約は置き換え・取り消し後の発火は無視、モジュール doc 参照）。
//!
//! `schedule()` は FFI 呼び出しより先に `state` を `Scheduled` にする
//! （発火が状態確定より先に届いて実行が失われるのを防ぐ。FFI が失敗したときだけ、
//! 同じ世代に限って `NotScheduled` へ戻す）。
//!
//! 置き換え前に `Scheduled` な予約が生きていた場合は、その世代を
//! [`Inner::pending_replacement`] に退避してから `generation` を進める。FFI の
//! 成功・失敗が確定する前（`.m` 側が旧 scheduler をまだ生かしている窓）に旧世代の
//! 発火が届いても、[`pending_replacement`](Inner::pending_replacement) 経由で
//! 記録を取りこぼさない（置き換えが最終的に失敗した場合にのみ `last_run` へ
//! 反映する。置き換え失敗時に旧世代へ戻すだけでは、この窓で届いた発火が
//! 捨てられたまま復元できないため、`device-bridge-contracts.md`
//! 「2. 並行性・世代」の構造的欠陥になる。PR #1048 レビュー指摘）。
//!
//! # ロック順序
//!
//! `ffi`（schedule/cancel の呼び出しを直列化する）→ `state`（判定・更新）の順で
//! 取る。発火コールバック（[`fandhe_rust_background_task_fired`]）は `state` だけを
//! 取り `ffi` には触れない。`.m` 側の `invalidate` と発火コールバックが同じ
//! `NSBackgroundActivityScheduler` の private queue 上で直列に実行される契約
//! （`.m` ファイル冒頭コメント参照）のため、ここで `ffi` まで取るとコールバック側の
//! 完了を待つ `invalidate` 呼び出しと相互に待ち合ってデッドロックしうる。
//!
//! # アプリ終了時
//!
//! プロセスが終わると `NSBackgroundActivityScheduler` を含め全リソースが消えるため、
//! `device::shutdown` への追加は不要（`.m` ファイル冒頭コメントと対応）。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use fandhe_device::background_task::{
    read_record, write_record_if_newer, BackgroundTask, RunRecord, RunResult, ScheduleState,
};
use fandhe_device::storage::Storage;
use fandhe_device::DeviceError;

/// 予約の調整用定数（実機の遅延に合わせて調整できるよう定数にしておく）。
const INTERVAL_SECS: f64 = 10.0;
const TOLERANCE_SECS: f64 = 5.0;

// SAFETY: `macos/fandhe_macos_background_task.m`（`build.rs` が macOS ターゲットの
// feature `device` 有効時にコンパイル・リンクする）が定義する C ABI。契約は `.m`
// ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_macos_background_task_schedule(generation: u64, interval: f64, tolerance: f64)
        -> i32;
    fn fandhe_macos_background_task_cancel();
}

mod ffi {
    /// `.m` 側の予約依頼を呼ぶ（`MacBackgroundTask::schedule_fn` の既定実装。
    /// テストから差し替えられるよう関数ポインタとして保持される、モジュール doc 参照）。
    #[allow(unsafe_code)]
    pub(super) fn schedule(generation: u64, interval: f64, tolerance: f64) -> bool {
        // SAFETY: 引数はプリミティブのみでポインタを経由しない。
        let code = unsafe {
            super::fandhe_macos_background_task_schedule(generation, interval, tolerance)
        };
        code == 0
    }

    /// `.m` 側の取り消しを呼ぶ（`MacBackgroundTask::cancel_fn` の既定実装）。
    #[allow(unsafe_code)]
    pub(super) fn cancel() {
        // SAFETY: 引数なし、戻り値なし。
        unsafe { super::fandhe_macos_background_task_cancel() };
    }
}

/// `state` フィールドが保持する内部状態（モジュール doc「世代管理」参照）。
struct Inner {
    /// `schedule`/`cancel` のたびに進む世代番号。
    generation: u64,
    /// 現在の予約状態（`ScheduleState` そのまま）。
    state: ScheduleState,
    /// 直近の `last_run` 保存が失敗したか（`true` の間は `last_run()` が `Err` を
    /// 返す。保存失敗を「未実行」へ畳まない、`device-bridge-contracts.md`
    /// 「6. 嘘の成功」参照）。
    write_failed: bool,
    /// 置き換え中の旧世代（モジュール doc「世代管理」参照）。`schedule()` が
    /// FFI 呼び出し前に置き換え前の世代を控えておき、FFI の成否が確定する
    /// までの窓に届いた旧世代の発火を [`on_fired`] がここへ記録する。置き換えが
    /// 失敗したときだけ `schedule()` がこれを読んで `state`/`write_failed` へ
    /// 反映する（成功時は新しい予約の状態がそのまま有効なので読み捨てる。
    /// どちらの場合も `write_record` 自体は発火時点で確定済みなので記録は失われない）。
    pending_replacement: Option<PendingReplacement>,
}

/// 置き換え中に発火した旧世代の記録（[`Inner::pending_replacement`] 参照）。
struct PendingReplacement {
    /// 置き換え前に生きていた世代番号。
    generation: u64,
    /// この世代が置き換え確定前に発火したときの `write_record` 結果
    /// （`write_failed`）。`None` はまだ発火していないことを表す。
    fired_write_failed: Option<bool>,
}

/// `NSBackgroundActivityScheduler` による macOS 向け `BackgroundTask` 実装。
pub struct MacBackgroundTask {
    state: Mutex<Inner>,
    /// `schedule`/`cancel` の FFI 呼び出しを直列化する（モジュール doc「ロック順序」）。
    ffi: Mutex<()>,
    storage: Arc<dyn Storage>,
    schedule_fn: fn(u64, f64, f64) -> bool,
    cancel_fn: fn(),
}

impl MacBackgroundTask {
    /// 既定の FFI 実装で構築する（`device/background_task.rs::implementation` から
    /// 呼ばれる契約）。
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self::with_ffi(storage, ffi::schedule, ffi::cancel)
    }

    /// テストから任意の FFI 実装を注入する入口（`apple_mic_record.rs` 等と同じ設計）。
    fn with_ffi(
        storage: Arc<dyn Storage>,
        schedule_fn: fn(u64, f64, f64) -> bool,
        cancel_fn: fn(),
    ) -> Self {
        Self {
            state: Mutex::new(Inner {
                generation: 0,
                state: ScheduleState::NotScheduled,
                write_failed: false,
                pending_replacement: None,
            }),
            ffi: Mutex::new(()),
            storage,
            schedule_fn,
            cancel_fn,
        }
    }
}

impl BackgroundTask for MacBackgroundTask {
    fn schedule(&self) -> Result<(), DeviceError> {
        let _ffi_guard = self.ffi.lock().expect("lock は成功する");

        // FFI 呼び出しより先に `Scheduled` を立てる（発火が状態確定より先に届いて
        // 実行が失われるのを防ぐ、モジュール doc 参照）。失敗時に戻せるよう、
        // 置き換え前の世代・状態も退避しておく（`.m` 側は新しい scheduler の生成に
        // 失敗した場合、既存の予約には触れず残す契約なので、Rust 側の状態もその
        // 旧予約に合わせて戻す必要がある。`device-bridge-contracts.md`
        // 「9. 失敗時のリソース解放・共有状態復元」参照）。
        let (generation, previous_generation, previous_state) = {
            let mut inner = self.state.lock().expect("lock は成功する");
            let previous_generation = inner.generation;
            let previous_state = inner.state;
            // 置き換え前に生きている予約があれば、FFI の成否が確定するまでの
            // 窓で届く旧世代の発火を取りこぼさないよう控えておく
            // （モジュール doc「世代管理」参照）。
            inner.pending_replacement =
                (previous_state == ScheduleState::Scheduled).then_some(PendingReplacement {
                    generation: previous_generation,
                    fired_write_failed: None,
                });
            inner.generation = inner.generation.wrapping_add(1);
            inner.state = ScheduleState::Scheduled;
            (inner.generation, previous_generation, previous_state)
        };

        let ok = (self.schedule_fn)(generation, INTERVAL_SECS, TOLERANCE_SECS);

        let mut inner = self.state.lock().expect("lock は成功する");
        // 窓の間に旧世代が発火していれば、その結果（`write_record` の成否）を
        // ここで回収する。一致しない（別の世代の `pending_replacement` が既に
        // 積まれている＝さらに別の schedule/cancel が割り込んだ）場合は無視する。
        let fired_during_transition = inner
            .pending_replacement
            .take()
            .filter(|pending| pending.generation == previous_generation)
            .and_then(|pending| pending.fired_write_failed);

        if ok {
            // 置き換え成功後も、窓の間に旧予約が発火して `write_record` に
            // 失敗していた場合はその失敗を捨てない。新しい予約が有効に
            // なった後も、直近の発火が保存できなかった事実自体は変わらない
            // ため、`last_run()` が `Err` を返し続けるよう反映する
            // （`device-bridge-contracts.md`「6. 嘘の成功」、PR #1048 レビュー指摘）。
            // ただし FFI 復帰後この lock を取るまでに新しい予約自身が既に発火して
            // いれば（`state` が `Scheduled` でない）、そちらの方が新しい結果なので
            // 旧予約の結果で上書きしない（PR #1048 codex 指摘）。
            let new_fired =
                inner.generation == generation && inner.state != ScheduleState::Scheduled;
            if let (Some(write_failed), false) = (fired_during_transition, new_fired) {
                inner.write_failed = write_failed;
            }
            return Ok(());
        }

        // FFI 失敗: 同じ世代のときだけ、置き換え前の世代・状態へ戻す（その間に
        // 別の schedule/cancel が割り込んでいたら、その世代の状態を壊さない）。
        // 旧予約（`.m` 側の `g_scheduler`）はそのまま生き続けるため、世代も
        // 旧世代へ戻すことで、その発火を `on_fired` が正しく受理できるようにする。
        if inner.generation == generation {
            inner.generation = previous_generation;
            match fired_during_transition {
                // 旧予約は置き換え確定前に既に発火していた。取りこぼさず
                // `Ran` として確定する（`write_record` 自体は発火時点で
                // 確定済みなので `write_failed` もその結果をそのまま引き継ぐ）。
                Some(write_failed) => {
                    inner.state = ScheduleState::Ran;
                    inner.write_failed = write_failed;
                }
                None => inner.state = previous_state,
            }
        }
        Err(DeviceError::Internal("background task schedule"))
    }

    fn cancel(&self) -> Result<(), DeviceError> {
        let _ffi_guard = self.ffi.lock().expect("lock は成功する");
        {
            let mut inner = self.state.lock().expect("lock は成功する");
            inner.generation = inner.generation.wrapping_add(1);
            inner.state = ScheduleState::NotScheduled;
            // 進行中の置き換え窓があっても取り消しで打ち切る（窓の間の発火は
            // 既に `on_fired` が記録済みなら消えない。未発火ならもう受理不要）。
            inner.pending_replacement = None;
        }
        (self.cancel_fn)();
        // 未予約での取り消しもべき等に `Ok`（モジュール doc 参照）。
        Ok(())
    }

    fn state(&self) -> Result<ScheduleState, DeviceError> {
        Ok(self.state.lock().expect("lock は成功する").state)
    }

    fn last_run(&self) -> Result<Option<RunRecord>, DeviceError> {
        if self.state.lock().expect("lock は成功する").write_failed {
            // 保存失敗を「未実行」に見せない（`device-bridge-contracts.md`「6. 嘘の成功」）。
            return Err(DeviceError::Internal("background task record write"));
        }
        read_record(self.storage.as_ref())
    }
}

/// 発火コールバック本体（[`fandhe_rust_background_task_fired`] から呼ばれる、
/// テストから直接呼べるよう分離）。`ffi` ロックには触れない（モジュール doc
/// 「ロック順序」参照）。世代が一致し `Scheduled` のときだけ記録を保存する。
fn on_fired(task: &MacBackgroundTask, generation: u64, now_unix_ms: u64) {
    let mut inner = task.state.lock().expect("lock は成功する");
    if inner.generation != generation || inner.state != ScheduleState::Scheduled {
        // 現在の世代ではない。置き換え確定前の窓であれば（モジュール doc
        // 「世代管理」参照）、旧世代の発火として記録を取りこぼさず控える
        // （`schedule()` が置き換え失敗時にこれを読んで `state` へ反映する）。
        // 新しい予約自身が既に発火済み（`state` が `Scheduled` でない）なら、
        // そちらの記録の方が新しいので旧世代の記録で上書きしない（PR #1048 codex 指摘）。
        let newer_fired = inner.state != ScheduleState::Scheduled;
        if let Some(pending) = inner.pending_replacement.as_mut() {
            if pending.generation == generation
                && pending.fired_write_failed.is_none()
                && !newer_fired
            {
                let record = RunRecord {
                    ran_at_unix_ms: now_unix_ms,
                    result: RunResult::Completed,
                };
                // 既存の記録より古ければ書かない（`fandhe_device::background_task` 契約「実行記録は
                // 時刻の新しい方を残す」）。
                let write_failed = write_record_if_newer(task.storage.as_ref(), &record).is_err();
                pending.fired_write_failed = Some(write_failed);
            }
        }
        // それ以外（キャンセル済み・さらに前の世代）の発火は捨てる。
        return;
    }
    let record = RunRecord {
        ran_at_unix_ms: now_unix_ms,
        result: RunResult::Completed,
    };
    // 書き込み成功可否と状態更新を同じロックの内側で確定させる
    // （`device-bridge-contracts.md`「2. 並行性・世代」）。
    match write_record_if_newer(task.storage.as_ref(), &record) {
        Ok(()) => {
            inner.state = ScheduleState::Ran;
            inner.write_failed = false;
        }
        Err(_) => {
            inner.state = ScheduleState::NotScheduled;
            inner.write_failed = true;
        }
    }
}

/// プロセス static（`device/background_task.rs::implementation` が構築した
/// 唯一のインスタンスを保持する。発火コールバックは FFI 境界を越えて届くため
/// `Arc` 経由でしか参照できない、`apple_biometric.rs` の `g_*` static と同じ必要性）。
static INSTANCE: OnceLock<Arc<MacBackgroundTask>> = OnceLock::new();

/// `device/background_task.rs::implementation` から 1 度だけ呼ばれ、以後の発火
/// コールバックが参照する static を確定させる。
pub(crate) fn install(task: Arc<MacBackgroundTask>) -> Arc<MacBackgroundTask> {
    INSTANCE.get_or_init(|| task).clone()
}

/// `.m` 側の block から呼ばれる `extern "C"` エクスポート関数（`.m` ファイル冒頭
/// コメント参照）。[`INSTANCE`] が未設定（`install` 未呼び出し）の間に届いた場合は
/// 何もしない（構築前に発火することはない契約だが、防御的に無視する）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_background_task_fired(generation: u64) {
    let Some(task) = INSTANCE.get() else {
        return;
    };
    let now_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    on_fired(task, generation, now_unix_ms);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
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

    static ALWAYS_OK_CALLS: AtomicU64 = AtomicU64::new(0);
    fn always_ok(_generation: u64, _interval: f64, _tolerance: f64) -> bool {
        ALWAYS_OK_CALLS.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn always_fail(_generation: u64, _interval: f64, _tolerance: f64) -> bool {
        false
    }
    static CANCEL_CALLS: AtomicU64 = AtomicU64::new(0);
    fn noop_cancel() {
        CANCEL_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    fn task_with(
        schedule_fn: fn(u64, f64, f64) -> bool,
        storage: Arc<dyn Storage>,
    ) -> MacBackgroundTask {
        MacBackgroundTask::with_ffi(storage, schedule_fn, noop_cancel)
    }

    #[test]
    fn schedule_success_sets_scheduled() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        assert!(task.schedule().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::Scheduled);
    }

    #[test]
    fn schedule_failure_reverts_to_not_scheduled() {
        let task = task_with(always_fail, Arc::new(MemStorage::default()));
        assert!(task.schedule().is_err());
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
    }

    static SUCCEED_ONCE_CALLS: AtomicU64 = AtomicU64::new(0);
    fn succeed_once_then_fail(_generation: u64, _interval: f64, _tolerance: f64) -> bool {
        SUCCEED_ONCE_CALLS.fetch_add(1, Ordering::SeqCst) == 0
    }

    #[test]
    fn schedule_failure_after_prior_success_restores_previous_reservation() {
        // 置き換え（2 回目）の `schedule_fn` が失敗するケース。`.m` 側は新しい
        // scheduler の生成に失敗した場合、既存の（1 回目の）予約には触れず残す
        // 契約なので、Rust 側の世代・状態も 1 回目のものへ戻り、その発火が
        // 捨てられず受理できる必要がある（PR #1048 レビュー指摘の回帰テスト）。
        let task = task_with(succeed_once_then_fail, Arc::new(MemStorage::default()));
        assert!(task.schedule().is_ok());
        let first_generation = task.state.lock().unwrap().generation;
        assert_eq!(task.state().unwrap(), ScheduleState::Scheduled);

        assert!(task.schedule().is_err());
        assert_eq!(task.state.lock().unwrap().generation, first_generation);
        assert_eq!(task.state().unwrap(), ScheduleState::Scheduled);

        on_fired(&task, first_generation, 1);
        assert_eq!(task.state().unwrap(), ScheduleState::Ran);
    }

    static RACE_CALLS: AtomicU64 = AtomicU64::new(0);
    static RACE_CALLBACK: StdMutex<Option<Box<dyn FnMut() + Send>>> = StdMutex::new(None);
    fn succeed_once_then_fire_old_and_fail(
        _generation: u64,
        _interval: f64,
        _tolerance: f64,
    ) -> bool {
        if RACE_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            return true;
        }
        if let Some(cb) = RACE_CALLBACK.lock().unwrap().as_mut() {
            cb();
        }
        false
    }

    #[test]
    fn fired_during_in_flight_replacement_is_not_lost_when_replacement_fails() {
        // 既存予約がある状態で `schedule()` を呼ぶと、FFI の成功が確定する前に
        // `generation` を進める。その窓の間に旧予約（置き換えられる予定の予約）が
        // 発火し、かつ置き換え自体が失敗するケース。旧世代の発火を単純に世代
        // 不一致として捨てると、置き換え失敗で世代を旧予約へ戻しても、実際に
        // 行われた実行が `last_run()` に残らない（PR #1048 codex 指摘の回帰
        // テスト、device-bridge-contracts.md「2. 並行性・世代」）。
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let task = Arc::new(MacBackgroundTask::with_ffi(
            storage,
            succeed_once_then_fire_old_and_fail,
            noop_cancel,
        ));
        assert!(task.schedule().is_ok()); // 1 回目: generation = 1, Scheduled
        let first_generation = task.state.lock().unwrap().generation;

        let task_for_cb = Arc::clone(&task);
        *RACE_CALLBACK.lock().unwrap() = Some(Box::new(move || {
            // 2 回目の schedule() が generation を進めた直後、FFI 結果が
            // 確定する前に旧世代（1 回目）の発火が割り込む想定。
            on_fired(&task_for_cb, first_generation, 9_000);
        }));

        assert!(task.schedule().is_err()); // 2 回目: 置き換えに失敗する

        // 置き換え失敗なので世代は旧予約へ戻り、かつ窓の間に発火した実行が
        // 失われず Ran として確定している必要がある。
        assert_eq!(task.state.lock().unwrap().generation, first_generation);
        assert_eq!(task.state().unwrap(), ScheduleState::Ran);
        let record = task
            .last_run()
            .unwrap()
            .expect("発火した記録が保存されている");
        assert_eq!(record.ran_at_unix_ms, 9_000);
    }

    static RACE_SUCCESS_CALLS: AtomicU64 = AtomicU64::new(0);
    static RACE_SUCCESS_CALLBACK: StdMutex<Option<Box<dyn FnMut() + Send>>> = StdMutex::new(None);
    fn succeed_once_then_fire_old_and_succeed(
        _generation: u64,
        _interval: f64,
        _tolerance: f64,
    ) -> bool {
        if RACE_SUCCESS_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            return true;
        }
        if let Some(cb) = RACE_SUCCESS_CALLBACK.lock().unwrap().as_mut() {
            cb();
        }
        true
    }

    #[test]
    fn fired_during_in_flight_replacement_write_failure_is_not_lost_when_replacement_succeeds() {
        // 置き換え（2 回目）の `schedule_fn` が成功するケース。その窓の間に旧予約が
        // 発火し `write_record` に失敗していた場合、新しい予約が有効になった後も
        // その保存失敗を捨てて `Ok(())` に畳んではならない（PR #1048 レビュー指摘
        // の回帰テスト、device-bridge-contracts.md「6. 嘘の成功」）。
        let storage: Arc<dyn Storage> = Arc::new(FailingWriteStorage);
        let task = Arc::new(MacBackgroundTask::with_ffi(
            storage,
            succeed_once_then_fire_old_and_succeed,
            noop_cancel,
        ));
        assert!(task.schedule().is_ok()); // 1 回目: generation = 1, Scheduled
        let first_generation = task.state.lock().unwrap().generation;

        let task_for_cb = Arc::clone(&task);
        *RACE_SUCCESS_CALLBACK.lock().unwrap() = Some(Box::new(move || {
            // 2 回目の schedule() が generation を進めた直後、FFI 結果が
            // 確定する前に旧世代（1 回目）の発火が割り込む想定。
            on_fired(&task_for_cb, first_generation, 9_000);
        }));

        assert!(task.schedule().is_ok()); // 2 回目: 置き換えに成功する

        // 置き換え成功なので state は新しい予約の Scheduled のまま進むが、
        // 窓の間に発火した旧予約の保存失敗は last_run() から読み取れる必要がある。
        assert_eq!(task.state().unwrap(), ScheduleState::Scheduled);
        assert!(task.last_run().is_err());
    }

    /// 1 回目の書き込みだけ失敗し、以後は成功するストレージ。
    #[derive(Default)]
    struct FailOnceStorage {
        writes: AtomicU64,
        inner: MemStorage,
    }

    impl Storage for FailOnceStorage {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
            self.inner.read(key)
        }

        fn write(&self, key: &str, bytes: &[u8]) -> Result<(), DeviceError> {
            if self.writes.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(DeviceError::Internal("write failure"));
            }
            self.inner.write(key, bytes)
        }
    }

    static NEWER_CALLS: AtomicU64 = AtomicU64::new(0);
    static NEWER_CALLBACK: StdMutex<Option<Box<dyn FnMut() + Send>>> = StdMutex::new(None);
    fn succeed_once_then_fire_old_and_new(
        _generation: u64,
        _interval: f64,
        _tolerance: f64,
    ) -> bool {
        if NEWER_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            return true;
        }
        if let Some(cb) = NEWER_CALLBACK.lock().unwrap().as_mut() {
            cb();
        }
        true
    }

    #[test]
    fn newer_successful_fire_is_not_overwritten_by_old_write_failure() {
        // 置き換え窓の間に旧予約が発火して保存に失敗し、続けて新しい予約自身も
        // 発火して保存に成功したケース。新しい結果の方が新しいので、旧予約の
        // 失敗で `last_run()` を Err に戻してはならない（PR #1048 codex 指摘）。
        let task = Arc::new(MacBackgroundTask::with_ffi(
            Arc::new(FailOnceStorage::default()),
            succeed_once_then_fire_old_and_new,
            noop_cancel,
        ));
        assert!(task.schedule().is_ok());
        let first_generation = task.state.lock().unwrap().generation;

        let task_for_cb = Arc::clone(&task);
        *NEWER_CALLBACK.lock().unwrap() = Some(Box::new(move || {
            on_fired(&task_for_cb, first_generation, 9_000); // 旧: 保存失敗
            on_fired(&task_for_cb, first_generation + 1, 9_500); // 新: 保存成功
        }));

        assert!(task.schedule().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::Ran);
        let record = task.last_run().unwrap().expect("新しい発火の記録");
        assert_eq!(record.ran_at_unix_ms, 9_500);
    }

    static NEW_FIRST_CALLS: AtomicU64 = AtomicU64::new(0);
    static NEW_FIRST_CALLBACK: StdMutex<Option<Box<dyn FnMut() + Send>>> = StdMutex::new(None);
    fn succeed_once_then_fire_new_and_old(
        _generation: u64,
        _interval: f64,
        _tolerance: f64,
    ) -> bool {
        if NEW_FIRST_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            return true;
        }
        if let Some(cb) = NEW_FIRST_CALLBACK.lock().unwrap().as_mut() {
            cb();
        }
        true
    }

    #[test]
    fn late_old_fire_does_not_overwrite_newer_record() {
        // 置き換え窓の間に新しい予約が先に発火・保存し、その後で旧予約が発火した
        // ケース。旧世代の記録で永続化済みの新しい記録を巻き戻さない（PR #1048 codex 指摘）。
        let task = Arc::new(MacBackgroundTask::with_ffi(
            Arc::new(MemStorage::default()),
            succeed_once_then_fire_new_and_old,
            noop_cancel,
        ));
        assert!(task.schedule().is_ok());
        let first_generation = task.state.lock().unwrap().generation;

        let task_for_cb = Arc::clone(&task);
        *NEW_FIRST_CALLBACK.lock().unwrap() = Some(Box::new(move || {
            on_fired(&task_for_cb, first_generation + 1, 9_500); // 新
            on_fired(&task_for_cb, first_generation, 9_000); // 旧（遅れて届く）
        }));

        assert!(task.schedule().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::Ran);
        let record = task.last_run().unwrap().expect("新しい発火の記録");
        assert_eq!(record.ran_at_unix_ms, 9_500);
    }

    #[test]
    fn older_fire_does_not_overwrite_newer_saved_record() {
        // 既に新しい時刻の記録があるとき、古い時刻の発火で巻き戻さない
        // （`fandhe_device::background_task` 契約「実行記録は時刻の新しい方を残す」、
        // PR #1053 codex 指摘）。
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        fandhe_device::background_task::write_record(
            storage.as_ref(),
            &RunRecord {
                ran_at_unix_ms: 2_000,
                result: RunResult::Completed,
            },
        )
        .expect("書き込みは成功する");
        let task = task_with(always_ok, storage);
        assert!(task.schedule().is_ok());
        let generation = task.state.lock().unwrap().generation;
        on_fired(&task, generation, 1_000);
        let record = task.last_run().unwrap().expect("記録がある");
        assert_eq!(record.ran_at_unix_ms, 2_000);
    }

    #[test]
    fn double_schedule_advances_generation_and_replaces() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        task.schedule().unwrap();
        let first_generation = task.state.lock().unwrap().generation;
        task.schedule().unwrap();
        let second_generation = task.state.lock().unwrap().generation;
        assert_eq!(second_generation, first_generation + 1);
        assert!(ALWAYS_OK_CALLS.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn cancel_without_prior_schedule_is_ok_and_does_not_change_state() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
        assert!(task.cancel().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
    }

    #[test]
    fn cancel_after_schedule_resets_to_not_scheduled() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        task.schedule().unwrap();
        assert!(task.cancel().is_ok());
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
    }

    #[test]
    fn fired_with_stale_generation_is_discarded() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        task.schedule().unwrap(); // generation = 1
        task.schedule().unwrap(); // generation = 2, 状態は Scheduled

        // 世代 1（古い）の発火は捨てられ、記録は保存されない。
        on_fired(&task, 1, 1_000);
        assert_eq!(task.state().unwrap(), ScheduleState::Scheduled);
        assert_eq!(task.last_run().unwrap(), None);
    }

    #[test]
    fn fired_after_cancel_is_discarded() {
        let task = task_with(always_ok, Arc::new(MemStorage::default()));
        task.schedule().unwrap(); // generation = 1
        task.cancel().unwrap(); // generation = 2, NotScheduled

        on_fired(&task, 1, 1_000);
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
        assert_eq!(task.last_run().unwrap(), None);
    }

    #[test]
    fn fired_with_current_generation_saves_record_and_sets_ran() {
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let task = task_with(always_ok, storage);
        task.schedule().unwrap(); // generation = 1

        on_fired(&task, 1, 42_000);
        assert_eq!(task.state().unwrap(), ScheduleState::Ran);
        let record = task.last_run().unwrap().expect("記録が保存されている");
        assert_eq!(record.ran_at_unix_ms, 42_000);
        assert_eq!(record.result, RunResult::Completed);
    }

    #[test]
    fn fired_write_failure_sets_last_run_err_not_never_ran() {
        let task = task_with(always_ok, Arc::new(FailingWriteStorage));
        task.schedule().unwrap(); // generation = 1

        on_fired(&task, 1, 1_000);
        assert_eq!(task.state().unwrap(), ScheduleState::NotScheduled);
        // 書き込み失敗は「未実行」(`Ok(None)`) に化けず `Err` になる
        // （`device-bridge-contracts.md`「6. 嘘の成功」）。
        assert!(task.last_run().is_err());
    }

    #[test]
    fn cancel_calls_ffi_cancel() {
        // `CANCEL_CALLS`/`noop_cancel` は他のテスト（`task_with` 経由）とも共有する
        // ため、ここでは専用の static を使う（`cargo test` の並列実行で他テストの
        // `cancel()` 呼び出しと競合し、差分比較が意図せず失敗することを避ける）。
        static DEDICATED_CANCEL_CALLS: AtomicU64 = AtomicU64::new(0);
        fn dedicated_cancel() {
            DEDICATED_CANCEL_CALLS.fetch_add(1, Ordering::SeqCst);
        }

        let task = MacBackgroundTask::with_ffi(
            Arc::new(MemStorage::default()),
            always_ok,
            dedicated_cancel,
        );
        task.cancel().unwrap();
        assert_eq!(DEDICATED_CANCEL_CALLS.load(Ordering::SeqCst), 1);
    }

    /// `write_failed` フラグが立っている間は、取り消し後も `last_run` が `Err` の
    /// ままであることを確認する（新しい予約を経ないと回復しない契約の固定）。
    #[test]
    fn write_failed_persists_until_next_successful_run() {
        let storage: Arc<dyn Storage> = Arc::new(FailingWriteStorage);
        let task = task_with(always_ok, storage);
        task.schedule().unwrap();
        // 一時変数へ取り出してからロックを解放する。`task.state.lock().unwrap()` を
        // そのまま引数式へ書くと、一時 `MutexGuard` の寿命が文末（`on_fired` 呼び出し
        // 完了後）まで延長され、`on_fired` 内部の再ロックで自己デッドロックする
        // （`std::sync::Mutex` は非再入のため）。
        let generation = task.state.lock().unwrap().generation;
        on_fired(&task, generation, 1);
        assert!(task.last_run().is_err());
        task.cancel().unwrap();
        assert!(task.last_run().is_err());
    }
}
