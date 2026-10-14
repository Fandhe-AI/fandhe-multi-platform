//! Windows 向けバックグラウンド処理実装（Issue #1044）。Task Scheduler 2.0 COM
//! （`ITaskService`/`ITaskFolder`/`ITaskDefinition`/`ITimeTrigger`/`IExecAction`）で、
//! 自分の実行ファイルを専用引数付きで 1 回だけ起動するタスクを現在ユーザーで登録する。
//! パッケージ化前提の `BackgroundTaskBuilder` は使わない（10-01 のユーザー判断）。
//!
//! `device/background_task.rs` の Windows 経路から [`WindowsBackgroundTask`] を使う
//! （`fandhe_device::background_task::BackgroundTask` 実装、`macos_background_task.rs`
//! と同型）。実行そのものは別プロセス（タスク起動時に専用引数付きで `main.rs` が
//! 再度立つ）が行うため発火コールバックは無く、[`WindowsBackgroundTask::state`] は
//! 保存済みの実行記録と自分が出した予約時刻を比較するだけで判定する
//! （下記「状態判定」節）。
//!
//! # 契約の簡略化（オーナー判断 10/2、`fandhe_device::background_task` モジュール
//! doc「契約」節参照）
//!
//! 予約・取り消し・実行の順序が入れ替わる競合条件を厳密に処理する契約は採らない。
//! 旧実装が持っていた「世代番号による子プロセスの照合」「世代発行とタスク登録を
//! プロセス間 named Mutex で直列化する」仕組みは削除した（削除理由は PR #1049 の
//! レビュー履歴を参照。named Mutex のプロセス間ロック取得失敗時の fail-open・
//! 取り消しとの排他漏れ・世代永続化の失敗無視・昇格時の `CreateMutexW` 失敗は、
//! いずれもこの仕組み自体を無くすことで解消する）。代わりに次の 3 点に単純化する。
//!
//! - **子プロセスの引数**: [`task_arguments`] は固定フラグのみ（世代番号を含まない）。
//!   [`run_scheduled`] は登録済みタスク定義との照合をせず、起動されたら常に実行結果
//!   を書く
//! - **実行記録**: [`fandhe_device::background_task::write_record_if_newer`] で
//!   「時刻の新しい方を残す」。旧世代の子プロセスが遅れて発火しても、既に新しい
//!   記録が書かれていれば上書きしない（厳密な世代照合ではなく、この程度の保護で
//!   十分という契約）
//! - **予約状態**: [`WindowsBackgroundTask::state`] はプロセス内に持つ直近の予約
//!   時刻と保存済み記録を比較する参考値。COM へは問い合わせない（モジュール doc
//!   「状態判定」節参照）。正確さは保証しない
//!
//! # 残骸の防止（`device-bridge-contracts.md`「9. 失敗時のリソース解放」）
//!
//! `ITimeTrigger::EndBoundary`（開始 + 1 日）と `ITaskSettings::
//! SetDeleteExpiredTaskAfter("PT0S")` を設定する。実行ファイルの移動等で子プロセスが
//! 起動しなかった場合に加え、`run_scheduled` が正常終了した後の発火済みタスクの
//! 回収もこの仕組みに委ねる（子プロセスからの `DeleteTask` は呼ばない。削除直前に
//! 親プロセスが `schedule()` で新しい予約へ置き換える TOCTOU を避けるため。単発
//! トリガーのため再発火はせず、次の `schedule()`/`cancel()` が同じ固定タスク名を
//! 置き換え・削除するか、`EndBoundary`/`SetDeleteExpiredTaskAfter` が最長 1 日後に
//! 自動回収する）。`SetExecutionTimeLimit("PT1M")` で子プロセスのハングも OS 側の
//! 期限で止める。`RegisterTaskDefinition` の登録自体は原子的（`TASK_CREATE_OR_UPDATE`）
//! なため、失敗時に中途半端な定義が残らない。
//!
//! # 状態判定（UI スレッドからのポーリング、`device/background_task.rs::drive_poll`）
//!
//! `state()`/`last_run()` はフレームごとに呼ばれるため COM を呼ばない（UI スレッドが
//! STA の可能性があり、`schedule`/`cancel` 側の MTA 初期化と衝突させないため）。
//! [`WindowsBackgroundTask`] が持つ直近の予約時刻（`scheduled_at_ms`）と、保存済みの
//! 実行記録（`fandhe_device::background_task::read_record`、共通 key）を比較する
//! だけで判定する: 予約なし → `NotScheduled`、記録の `ran_at_unix_ms` が予約時刻
//! 以降 → `Ran`、それ以外で現在時刻が予約時に記録した `EndBoundary` 未満 →
//! `Scheduled`、以上（Task Scheduler が `SetDeleteExpiredTaskAfter` で自動削除済みの
//! 見込み）→ `NotScheduled`。この判定は参考値であり、OS 側の実際の状態との食い違い
//! を正確に解消する契約は採らない（モジュール doc「契約の簡略化」節参照）。
//!
//! アプリ再起動後は直近の予約時刻（`inner`）が失われるため、再予約するまでは
//! `NotScheduled` を返す（trait 契約で許容。OS 側のタスクは残り、発火すれば記録
//! される）。
//!
//! # ログオン種別（設計判断）
//!
//! `TASK_LOGON_INTERACTIVE_TOKEN` + `TASK_RUNLEVEL_LUA`（userid/password は空
//! `VARIANT` ＝現在ユーザー）。パスワード保存方式（`TASK_LOGON_PASSWORD`）は
//! 採らない。`StartWhenAvailable` を立てるため、ログオフ中に取りこぼした実行は
//! 予約から 1 日（`EndBoundary`）以内に再ログオンした場合だけ
//! その時点で発火する（期限を過ぎるとタスクは削除され発火しない）。
//! 同一マシンの複数ユーザーがそれぞれ本アプリを動かすと
//! タスク名（ルートフォルダ固定名）が衝突し、後から登録した側は
//! `PermissionDenied`（`E_ACCESSDENIED`）になる（嘘の成功にはならない、
//! `ponytail:` 必要になったらユーザー名接尾辞を付ける）。
//!
//! `unsafe` は [`com`] サブモジュールの COM 呼び出しに限り、各呼び出しの直前に
//! `// SAFETY:` を付けて最小範囲の関数に閉じる（`lib.rs` の Windows
//! `deny(unsafe_code)` 緩和・coding-rust.md の FFI 例外規定準拠）。

use std::ffi::OsString;
use std::sync::{Arc, Mutex};

use fandhe_device::background_task::{read_record, BackgroundTask, RunRecord, ScheduleState};
use fandhe_device::storage::Storage;
use fandhe_device::DeviceError;
// `RunResult`/`write_record_if_newer` は [`run_scheduled`]（Windows ターゲットのみ）
// と本ファイルのテストから使うため、ホスト（非 Windows）の通常 lib ビルドでは未使用
// になる。`device-bridge-contracts.md`「7. 対応表・権限」の cfg 整合として、
// 利用箇所に合わせた cfg を import 自体にも付ける。
#[cfg(any(target_os = "windows", test))]
use fandhe_device::background_task::{write_record_if_newer, RunResult};

/// タスクの置き場・名前（ルートフォルダ `\` に固定、設計判断表参照）。
pub(crate) const TASK_NAME: &str = "fandhe-egui-proxy.background-task";

/// 子プロセスへ渡す専用フラグ（[`task_arguments`]/[`parse_scheduled_args`] 参照）。
/// 世代番号を含まない固定文字列（モジュール doc「契約の簡略化」節参照）。
const BACKGROUND_ARG: &str = "--fandhe-background-task";

/// 予約時刻から開始境界までの遅延秒（macOS 実装の `INTERVAL_SECS` と揃える）。
const START_DELAY_SECS: u64 = 10;

/// `EndBoundary`/`DeleteExpiredTaskAfter` の予備として開始境界に加える猶予（1 日）。
const END_BOUNDARY_SECS: u64 = 86_400;

/// タスクの `IExecAction::Arguments` に渡す固定文字列（モジュール doc「契約の簡略化」
/// 節参照。世代番号は含まない）。
pub(crate) fn task_arguments() -> String {
    BACKGROUND_ARG.to_string()
}

/// [`task_arguments`] の逆変換。`main.rs` が `std::env::args_os()` から渡した引数列が
/// `[フラグ]` の完全一致のときだけ `true` を返す（入力検証、security.md A03）。
pub fn parse_scheduled_args(args: &[OsString]) -> bool {
    matches!(args, [flag] if flag == BACKGROUND_ARG)
}

/// 1970-01-01 からの経過秒 → `ITrigger` の境界文字列（`YYYY-MM-DDTHH:MM:SS+00:00`、
/// `ITrigger::StartBoundary`/`EndBoundary` の文書化された書式）。現地時刻への変換を
/// 避けるため UTC 固定・明示オフセット付きで組み立てる（`fandhe_core::screens::
/// background_task::civil_from_days` を再利用する、日付計算を複製しない）。
fn format_boundary(total_secs: u64) -> String {
    let days = (total_secs / 86_400) as i64;
    let secs_of_day = total_secs % 86_400;
    let (y, m, d) = fandhe_core::screens::background_task::civil_from_days(days);
    let h = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}+00:00")
}

/// `scheduled_at_ms` → `(開始秒, 開始境界文字列, 終了境界文字列)`。開始は
/// `ceil(scheduled_at_ms / 1000) + START_DELAY_SECS`、終了は開始 + 1 日
/// （設計判断表「開始時刻」参照）。
fn start_boundary(scheduled_at_ms: u64) -> (u64, String, String) {
    let start_secs = scheduled_at_ms.div_ceil(1000) + START_DELAY_SECS;
    let end_secs = start_secs + END_BOUNDARY_SECS;
    (
        start_secs,
        format_boundary(start_secs),
        format_boundary(end_secs),
    )
}

/// `DeleteTask` が「タスクが無い」を表す HRESULT かどうか（`cancel` が、未予約・
/// 取り消し済みをべき等に扱うための判定。写像の前に行う、設計判断表参照）。
fn is_not_found(code: i32) -> bool {
    const ERROR_FILE_NOT_FOUND_HR: i32 = 0x8007_0002_u32 as i32;
    code == ERROR_FILE_NOT_FOUND_HR
}

/// HRESULT → 契約上のエラー種別への写像（SSOT。`schedule`/`cancel` の全呼び出し
/// 箇所がここを通す、`device-bridge-contracts.md`「5. FFI 境界」）。
fn map_hresult(code: i32) -> DeviceError {
    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    const SCHED_E_SERVICE_NOT_RUNNING: i32 = 0x8004_1315_u32 as i32;
    const ERROR_SERVICE_DISABLED_HR: i32 = 0x8007_0422_u32 as i32;
    const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154_u32 as i32;

    if code == E_ACCESSDENIED {
        DeviceError::PermissionDenied("background task")
    } else if code == SCHED_E_SERVICE_NOT_RUNNING || code == ERROR_SERVICE_DISABLED_HR {
        DeviceError::Disabled("background task")
    } else if code == REGDB_E_CLASSNOTREG {
        DeviceError::Unsupported("background task")
    } else {
        DeviceError::Internal("background task")
    }
}

/// 現在時刻（UNIX エポックからのミリ秒）。取得に失敗した場合は `0`
/// （`macos_background_task.rs::fandhe_rust_background_task_fired` と同じ方針。
/// 失敗は環境が壊れている場合のみで、`schedule`/`run_scheduled` の正しさが
/// 依存する唯一の性質は「単調に増える」ことではなく「現在時刻の近似」なので、
/// `0` へのフォールバックは安全側に倒れる――開始境界が過去になり得るが、
/// `ITimeTrigger` は過去の `StartBoundary` でも即時発火を試みるだけで、
/// 失敗や嘘の成功にはならない）。
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// [`WindowsBackgroundTask::inner`] が保持する予約の識別情報
/// （モジュール doc「状態判定」参照）。
#[derive(Clone, Copy)]
struct ScheduledEntry {
    /// このプロセスが最後に出した予約の時刻（`state()` が実行記録と比較する
    /// 参考値。モジュール doc「状態判定」参照）。
    scheduled_at_ms: u64,
    /// 予約の `EndBoundary`（モジュール doc「残骸の防止」参照）を UNIX エポック
    /// ミリ秒へ変換した値。COM を呼ばずに期限切れ（Task Scheduler による
    /// 自動削除後）を判定するための近似値。
    expires_at_ms: u64,
}

/// Task Scheduler 2.0 COM による Windows 向け `BackgroundTask` 実装。
pub struct WindowsBackgroundTask {
    /// このプロセスが最後に出した予約（`state()` の判定に使う参考値、モジュール doc
    /// 「状態判定」参照）。
    inner: Mutex<Option<ScheduledEntry>>,
    /// `schedule`/`cancel` の COM 呼び出しを直列化する（プロセス内のみ）。
    ffi: Mutex<()>,
    storage: Arc<dyn Storage>,
    register_fn: fn(u64) -> Result<(), i32>,
    delete_fn: fn() -> Result<(), i32>,
    now_ms_fn: fn() -> u64,
}

impl WindowsBackgroundTask {
    /// 既定の COM 実装で構築する（`device/background_task.rs::implementation` から
    /// 呼ばれる契約。構築時に COM は呼ばない）。
    #[cfg(target_os = "windows")]
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self::with_fns(storage, com::register, com::delete, now_unix_ms)
    }

    /// テストから任意の実装を注入する入口（`MacBackgroundTask::with_ffi` と同じ設計）。
    fn with_fns(
        storage: Arc<dyn Storage>,
        register_fn: fn(u64) -> Result<(), i32>,
        delete_fn: fn() -> Result<(), i32>,
        now_ms_fn: fn() -> u64,
    ) -> Self {
        Self {
            inner: Mutex::new(None),
            ffi: Mutex::new(()),
            storage,
            register_fn,
            delete_fn,
            now_ms_fn,
        }
    }
}

impl BackgroundTask for WindowsBackgroundTask {
    fn schedule(&self) -> Result<(), DeviceError> {
        let _ffi_guard = self.ffi.lock().expect("lock は成功する");
        let scheduled_at_ms = (self.now_ms_fn)();
        match (self.register_fn)(scheduled_at_ms) {
            Ok(()) => {
                // `TASK_CREATE_OR_UPDATE` による登録は原子的なため、置き換え前の
                // 旧予約は残らない（二重予約は置き換え契約）。
                let (start_secs, _, _) = start_boundary(scheduled_at_ms);
                let expires_at_ms = (start_secs + END_BOUNDARY_SECS) * 1_000;
                *self.inner.lock().expect("lock は成功する") = Some(ScheduledEntry {
                    scheduled_at_ms,
                    expires_at_ms,
                });
                Ok(())
            }
            // 登録失敗時は旧予約（あれば）をそのまま残す（`device-bridge-contracts.md`
            // 「9. 失敗時のリソース解放・共有状態復元」。嘘の成功を返さない）。
            Err(code) => Err(map_hresult(code)),
        }
    }

    fn cancel(&self) -> Result<(), DeviceError> {
        let _ffi_guard = self.ffi.lock().expect("lock は成功する");
        match (self.delete_fn)() {
            Ok(()) => {
                *self.inner.lock().expect("lock は成功する") = None;
                Ok(())
            }
            // 未予約での取り消しはべき等（モジュール doc「状態判定」参照）。
            Err(code) if is_not_found(code) => {
                *self.inner.lock().expect("lock は成功する") = None;
                Ok(())
            }
            Err(code) => Err(map_hresult(code)),
        }
    }

    fn state(&self) -> Result<ScheduleState, DeviceError> {
        let entry = *self.inner.lock().expect("lock は成功する");
        let Some(entry) = entry else {
            return Ok(ScheduleState::NotScheduled);
        };
        // 保存済みの実行記録（共通 key）が自分の予約時刻より後なら `Ran` とみなす
        // 参考値の判定（モジュール doc「契約の簡略化」節参照。正確さは保証しない）。
        // 同じミリ秒は「前の予約の記録」とみなし `Ran` にしない（実行は予約の
        // `START_DELAY_SECS` 後以降なので実運用では失うものが無い。PR #1049 codex 指摘）。
        if read_record(self.storage.as_ref())
            .ok()
            .flatten()
            .is_some_and(|record| record.ran_at_unix_ms > entry.scheduled_at_ms)
        {
            return Ok(ScheduleState::Ran);
        }
        // 未実行のまま `EndBoundary` を過ぎていれば、Task Scheduler が
        // `SetDeleteExpiredTaskAfter` で既に自動削除している見込みのため
        // `NotScheduled` へ倒す（COM は呼ばない。モジュール doc「状態判定」参照）。
        if (self.now_ms_fn)() >= entry.expires_at_ms {
            return Ok(ScheduleState::NotScheduled);
        }
        Ok(ScheduleState::Scheduled)
    }

    fn last_run(&self) -> Result<Option<RunRecord>, DeviceError> {
        // 共通 key（`fandhe_device::background_task::RECORD_KEY`）をそのまま読む。
        // 未存在は未実行、読み出し・形式解釈の失敗は `Err`（「last_run の 3 値」契約を
        // そのまま守る）。
        read_record(self.storage.as_ref())
    }
}

/// COM 呼び出しを閉じる（`unsafe` はこのサブモジュールに限る、モジュール doc 参照）。
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod com {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::{Interface, Result as WinResult, BSTR};
    use windows::Win32::Foundation::{VARIANT_FALSE, VARIANT_TRUE};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::TaskScheduler::{
        IExecAction, ITaskDefinition, ITaskFolder, ITaskService, ITimeTrigger, TaskScheduler,
        TASK_ACTION_EXEC, TASK_CREATE_OR_UPDATE, TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_LUA,
        TASK_TRIGGER_TIME,
    };
    use windows::Win32::System::Variant::VARIANT;

    use super::{start_boundary, task_arguments, TASK_NAME};

    /// COM 呼び出しを 1 つも行う前の失敗（`current_exe()` 等）に使う専用コード。
    /// 実在する COM エラーコードと衝突しない `E_FAIL` を流用する。
    const E_FAIL: i32 = 0x8000_4005_u32 as i32;

    /// `ITaskService` へ接続し、ルートフォルダ（`\`）を返す（`register`/`delete`
    /// 共通の入口）。
    fn connect() -> WinResult<(ITaskService, ITaskFolder)> {
        // SAFETY: `TaskScheduler`/`CLSCTX_INPROC_SERVER` は定数のみを渡す。戻り値は
        // `windows_core::Result` として失敗を呼び出し元へ伝える。
        let service: ITaskService =
            unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }?;
        let empty = VARIANT::default();
        // SAFETY: `service` は直前に取得した有効な COM ポインタ。4 引数すべて空
        // `VARIANT`（現在ユーザーでのローカル接続、設計判断表参照）。
        unsafe { service.Connect(&empty, &empty, &empty, &empty) }?;
        // SAFETY: `service` は接続済みの有効な COM ポインタ。パスは固定文字列。
        let folder = unsafe { service.GetFolder(&BSTR::from("\\")) }?;
        Ok((service, folder))
    }

    /// `exe` のパスを損失なく（UTF-8 経由の変換を経ずに）`BSTR` へ変換する
    /// （設計判断表「子プロセスの引数」参照。Windows のパスは常に有効な UTF-16
    /// だが、UTF-8 へのロスレス変換は単独サロゲート等で破綻しうる）。
    fn path_to_bstr(path: &Path) -> BSTR {
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        BSTR::from_wide(&wide)
    }

    /// `register` の本体。`ITaskDefinition` を 1 件組み立てて登録する。
    /// `scheduled_at_ms` は `ITimeTrigger` の開始境界計算（壁時計）に使う。
    fn register_inner(scheduled_at_ms: u64, exe: &Path) -> WinResult<()> {
        let (service, folder) = connect()?;
        // SAFETY: `service` は接続済みの有効な COM ポインタ。flags は常に 0
        // （予約値、`ITaskService::NewTask` の文書化された契約）。
        let def: ITaskDefinition = unsafe { service.NewTask(0) }?;

        // Principal: 最小権限（現在ユーザー・LUA、設計判断表「ログオン種別」参照）。
        // SAFETY: `def` は直前に取得した有効な COM ポインタ。
        let principal = unsafe { def.Principal() }?;
        // SAFETY: `principal` は直前に取得した有効な COM ポインタ。定数のみ渡す。
        unsafe { principal.SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN) }?;
        unsafe { principal.SetRunLevel(TASK_RUNLEVEL_LUA) }?;

        // Settings: 取りこぼし防止・残骸防止（モジュール doc「残骸の防止」参照）。
        // SAFETY: `def` は上と同じ有効な COM ポインタ。
        let settings = unsafe { def.Settings() }?;
        // SAFETY: `settings` は直前に取得した有効な COM ポインタ。定数・固定
        // 文字列のみ渡す。
        unsafe { settings.SetStartWhenAvailable(VARIANT_TRUE) }?;
        unsafe { settings.SetDisallowStartIfOnBatteries(VARIANT_FALSE) }?;
        unsafe { settings.SetStopIfGoingOnBatteries(VARIANT_FALSE) }?;
        unsafe { settings.SetExecutionTimeLimit(&BSTR::from("PT1M")) }?;
        unsafe { settings.SetDeleteExpiredTaskAfter(&BSTR::from("PT0S")) }?;

        // Trigger: 1 回だけのタイムトリガー。
        let (_, start, end) = start_boundary(scheduled_at_ms);
        // SAFETY: `def` は上と同じ有効な COM ポインタ。
        let triggers = unsafe { def.Triggers() }?;
        // SAFETY: `triggers` は直前に取得した有効な COM ポインタ。定数のみ渡す。
        let trigger = unsafe { triggers.Create(TASK_TRIGGER_TIME) }?;
        let time_trigger: ITimeTrigger = trigger.cast()?;
        // SAFETY: `time_trigger` は直前に取得した有効な COM ポインタ。文字列は
        // 自分で組み立てた固定書式（`start_boundary`）。
        unsafe { time_trigger.SetStartBoundary(&BSTR::from(start)) }?;
        unsafe { time_trigger.SetEndBoundary(&BSTR::from(end)) }?;

        // Action: 自分自身を専用引数で起動する。
        // SAFETY: `def` は上と同じ有効な COM ポインタ。
        let actions = unsafe { def.Actions() }?;
        // SAFETY: `actions` は直前に取得した有効な COM ポインタ。定数のみ渡す。
        let action = unsafe { actions.Create(TASK_ACTION_EXEC) }?;
        let exec: IExecAction = action.cast()?;
        // SAFETY: `exec` は直前に取得した有効な COM ポインタ。パスは呼び出し元
        // （`register`）が `current_exe()` から得た絶対パス、引数は固定フラグのみ
        // （security.md A03、モジュール doc 参照）。
        unsafe { exec.SetPath(&path_to_bstr(exe)) }?;
        unsafe { exec.SetArguments(&BSTR::from(task_arguments())) }?;

        // SAFETY: `folder`/`def` はいずれも有効な COM ポインタ。userid/password/
        // sddl はすべて空 `VARIANT`（現在ユーザー、設計判断表参照）。
        unsafe {
            folder.RegisterTaskDefinition(
                &BSTR::from(TASK_NAME),
                &def,
                TASK_CREATE_OR_UPDATE.0,
                &VARIANT::default(),
                &VARIANT::default(),
                TASK_LOGON_INTERACTIVE_TOKEN,
                &VARIANT::default(),
            )
        }?;
        Ok(())
    }

    /// `WindowsBackgroundTask::register_fn`（`schedule()` から呼ばれる）。
    pub(super) fn register(scheduled_at_ms: u64) -> Result<(), i32> {
        let exe = std::env::current_exe().map_err(|_| E_FAIL)?;
        // SAFETY: 呼び出しスレッドへの COM 初期化要求。`windows_notification::
        // ComInit` を再利用し、Drop で対になる `CoUninitialize` を呼ぶ
        // （モジュール doc 参照）。
        let _com = super::super::windows_notification::ComInit::new("background task schedule")
            .map_err(|_| E_FAIL)?;
        register_inner(scheduled_at_ms, &exe).map_err(|e| e.code().0)
    }

    /// `WindowsBackgroundTask::delete_fn`（`cancel()` から呼ばれる）。
    pub(super) fn delete() -> Result<(), i32> {
        let _com = super::super::windows_notification::ComInit::new("background task cancel")
            .map_err(|_| E_FAIL)?;
        delete_inner().map_err(|e| e.code().0)
    }

    fn delete_inner() -> WinResult<()> {
        let (_service, folder) = connect()?;
        // SAFETY: `folder` は接続済みの有効な COM ポインタ。パスは固定文字列、
        // flags は常に 0（予約値）。
        unsafe { folder.DeleteTask(&BSTR::from(TASK_NAME), 0) }
    }
}

/// 専用引数（[`parse_scheduled_args`]）付きで起動されたときの入口。`main.rs` の
/// 通常分岐より前に呼ばれ、画面を出さずに実行結果を書いて終了する（モジュール doc
/// 「契約の簡略化」節参照。登録済みタスク定義との世代照合はしない――起動された
/// 時点で実行とみなし、常に記録を書く）。戻り値はプロセスの終了コード
/// （`std::process::exit` へそのまま渡す契約）。
#[cfg(target_os = "windows")]
pub fn run_scheduled() -> i32 {
    let Ok(dir) = fandhe_device::storage::FsStorage::desktop_default() else {
        return 1;
    };
    let storage = fandhe_device::storage::FsStorage::new(dir);
    let record = RunRecord {
        ran_at_unix_ms: now_unix_ms(),
        result: RunResult::Completed,
    };
    // 「時刻の新しい方を残す」保護のみで、厳密な世代照合は行わない
    // （`fandhe_device::background_task::write_record_if_newer` 参照）。予約
    // （OS 側のタスク）はそのまま残す（タスク自体は削除しない。モジュール doc
    // 「残骸の防止」参照）。
    i32::from(write_record_if_newer(&storage, &record).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn task_arguments_round_trips_through_parse() {
        let args: Vec<OsString> = vec![OsString::from(task_arguments())];
        assert!(parse_scheduled_args(&args));
    }

    #[test]
    fn parse_scheduled_args_rejects_wrong_flag() {
        let args = [OsString::from("--other-flag")];
        assert!(!parse_scheduled_args(&args));
    }

    #[test]
    fn parse_scheduled_args_rejects_extra_argument() {
        let args = [OsString::from(BACKGROUND_ARG), OsString::from("extra")];
        assert!(!parse_scheduled_args(&args));
    }

    #[test]
    fn parse_scheduled_args_rejects_empty_args() {
        let args: [OsString; 0] = [];
        assert!(!parse_scheduled_args(&args));
    }

    #[test]
    fn start_boundary_adds_delay_and_rounds_up_to_the_second() {
        let (start_secs, start, end) = start_boundary(1_700_000_000_000);
        assert_eq!(start_secs, 1_700_000_010);
        assert_eq!(start, "2023-11-14T22:13:30+00:00");
        assert_eq!(end, "2023-11-15T22:13:30+00:00");
    }

    #[test]
    fn start_boundary_rounds_up_partial_milliseconds() {
        // 999ms 分は切り上げられ、開始は次の秒 + 10 秒になる。
        let (start_secs, _, _) = start_boundary(1_700_000_000_999);
        assert_eq!(start_secs, 1_700_000_011);
    }

    #[test]
    fn is_not_found_matches_only_file_not_found() {
        assert!(is_not_found(0x8007_0002_u32 as i32));
        assert!(!is_not_found(0x8007_0005_u32 as i32));
    }

    #[test]
    fn map_hresult_maps_access_denied_to_permission_denied() {
        assert!(matches!(
            map_hresult(0x8007_0005_u32 as i32),
            DeviceError::PermissionDenied("background task")
        ));
    }

    #[test]
    fn map_hresult_maps_service_not_running_to_disabled() {
        assert!(matches!(
            map_hresult(0x8004_1315_u32 as i32),
            DeviceError::Disabled("background task")
        ));
        assert!(matches!(
            map_hresult(0x8007_0422_u32 as i32),
            DeviceError::Disabled("background task")
        ));
    }

    #[test]
    fn map_hresult_maps_classnotreg_to_unsupported() {
        assert!(matches!(
            map_hresult(0x8004_0154_u32 as i32),
            DeviceError::Unsupported("background task")
        ));
    }

    #[test]
    fn map_hresult_maps_other_codes_to_internal() {
        assert!(matches!(
            map_hresult(-1),
            DeviceError::Internal("background task")
        ));
    }

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

    struct ErrOnReadStorage;

    impl Storage for ErrOnReadStorage {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
            Err(DeviceError::InvalidKey(key.to_string()))
        }

        fn write(&self, key: &str, _bytes: &[u8]) -> Result<(), DeviceError> {
            Err(DeviceError::InvalidKey(key.to_string()))
        }
    }

    fn task(
        storage: Arc<dyn Storage>,
        register_fn: fn(u64) -> Result<(), i32>,
        delete_fn: fn() -> Result<(), i32>,
        now_ms_fn: fn() -> u64,
    ) -> WindowsBackgroundTask {
        WindowsBackgroundTask::with_fns(storage, register_fn, delete_fn, now_ms_fn)
    }

    fn ok_register(_scheduled_at_ms: u64) -> Result<(), i32> {
        Ok(())
    }

    fn denied_register(_scheduled_at_ms: u64) -> Result<(), i32> {
        Err(0x8007_0005_u32 as i32)
    }

    fn ok_delete() -> Result<(), i32> {
        Ok(())
    }

    fn not_found_delete() -> Result<(), i32> {
        Err(0x8007_0002_u32 as i32)
    }

    fn denied_delete() -> Result<(), i32> {
        Err(0x8007_0005_u32 as i32)
    }

    fn now_1000() -> u64 {
        1_000
    }

    /// `inner` へ直接 `ScheduledEntry` を書き込む（`state()` の期限切れ判定を
    /// `schedule()`/COM から独立して検証するための private フィールドアクセス）。
    fn set_entry(t: &WindowsBackgroundTask, scheduled_at_ms: u64, expires_at_ms: u64) {
        *t.inner.lock().expect("lock は成功する") = Some(ScheduledEntry {
            scheduled_at_ms,
            expires_at_ms,
        });
    }

    #[test]
    fn schedule_success_reports_scheduled_then_ran_after_record() {
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let t = task(storage.clone(), ok_register, ok_delete, now_1000);
        t.schedule().expect("成功する");
        assert_eq!(t.state().expect("成功する"), ScheduleState::Scheduled);

        fandhe_device::background_task::write_record(
            storage.as_ref(),
            &RunRecord {
                ran_at_unix_ms: 11_000,
                result: RunResult::Completed,
            },
        )
        .expect("書き込みは成功する");
        assert_eq!(t.state().expect("成功する"), ScheduleState::Ran);
    }

    #[test]
    fn schedule_failure_is_mapped_and_state_stays_not_scheduled() {
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let t = task(storage, denied_register, ok_delete, now_1000);
        assert!(matches!(
            t.schedule(),
            Err(DeviceError::PermissionDenied("background task"))
        ));
        assert_eq!(t.state().expect("成功する"), ScheduleState::NotScheduled);
    }

    #[test]
    fn state_treats_expired_reservation_as_not_scheduled_without_com() {
        // 実行ファイルの移動等で子プロセスが起動できず発火しなかったまま
        // `EndBoundary` を過ぎると、Task Scheduler は `SetDeleteExpiredTaskAfter`
        // でタスクを自動削除する。`state()` は COM を呼ばないため、`schedule()`
        // 時に記録した期限（`expires_at_ms`）と現在時刻の比較だけで判定し、
        // `Scheduled` を返し続けない。
        fn now_past_expiry() -> u64 {
            2_000_000
        }
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let t = task(storage, ok_register, ok_delete, now_past_expiry);
        set_entry(&t, 1, 1_000_000);
        assert_eq!(t.state().expect("成功する"), ScheduleState::NotScheduled);
    }

    #[test]
    fn state_reports_scheduled_before_end_boundary() {
        fn now_before_expiry() -> u64 {
            500_000
        }
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let t = task(storage, ok_register, ok_delete, now_before_expiry);
        set_entry(&t, 1, 1_000_000);
        assert_eq!(t.state().expect("成功する"), ScheduleState::Scheduled);
    }

    #[test]
    fn cancel_without_prior_schedule_is_ok_even_when_not_found() {
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let t = task(storage, ok_register, not_found_delete, now_1000);
        assert!(t.cancel().is_ok());
        assert_eq!(t.state().expect("成功する"), ScheduleState::NotScheduled);
    }

    #[test]
    fn cancel_failure_is_mapped_and_schedule_state_is_preserved() {
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let t = task(storage, ok_register, denied_delete, now_1000);
        t.schedule().expect("成功する");
        assert!(matches!(
            t.cancel(),
            Err(DeviceError::PermissionDenied("background task"))
        ));
        // 取り消し失敗時は旧予約を残す（嘘の成功を返さない）。
        assert_eq!(t.state().expect("成功する"), ScheduleState::Scheduled);
    }

    #[test]
    fn double_schedule_replaces_the_reservation() {
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let t = task(storage.clone(), ok_register, ok_delete, now_1000);
        t.schedule().expect("成功する");
        // 予約と同じミリ秒の記録は前の予約のものとみなし `Ran` にしない。
        fandhe_device::background_task::write_record(
            storage.as_ref(),
            &RunRecord {
                ran_at_unix_ms: 1_000,
                result: RunResult::Completed,
            },
        )
        .expect("書き込みは成功する");
        assert_eq!(t.state().expect("成功する"), ScheduleState::Scheduled);
        fandhe_device::background_task::write_record(
            storage.as_ref(),
            &RunRecord {
                ran_at_unix_ms: 1_001,
                result: RunResult::Completed,
            },
        )
        .expect("書き込みは成功する");
        assert_eq!(t.state().expect("成功する"), ScheduleState::Ran);

        // 2 回目の schedule で予約が置き換わる。保存済みの実行記録は旧予約時刻
        // 以降のため `Ran` のままでも契約上問題ない（参考値、モジュール doc 参照）。
        t.schedule().expect("成功する");
        assert_eq!(t.state().expect("成功する"), ScheduleState::Ran);
    }

    #[test]
    fn last_run_distinguishes_read_failure_from_never_ran() {
        let storage: Arc<dyn Storage> = Arc::new(MemStorage::default());
        let t = task(storage, ok_register, ok_delete, now_1000);
        assert_eq!(t.last_run().expect("未実行は Ok(None)"), None);

        let failing: Arc<dyn Storage> = Arc::new(ErrOnReadStorage);
        let t = task(failing, ok_register, ok_delete, now_1000);
        assert!(t.last_run().is_err());
    }

    #[test]
    fn run_scheduled_keeps_newer_existing_record() {
        // `run_scheduled` は共通 key を使うため、`write_record_if_newer` の挙動を
        // ここでは直接確認する（`FsStorage` を要する `run_scheduled` 本体は
        // ホストテストから呼べないため）。
        let storage = MemStorage::default();
        let newer = RunRecord {
            ran_at_unix_ms: 2_000,
            result: RunResult::Completed,
        };
        fandhe_device::background_task::write_record(&storage, &newer).expect("書き込みは成功する");

        let older = RunRecord {
            ran_at_unix_ms: 1_000,
            result: RunResult::Completed,
        };
        write_record_if_newer(&storage, &older).expect("成功する（書かずに Ok）");
        assert_eq!(read_record(&storage).expect("read は成功する"), Some(newer));
    }
}
