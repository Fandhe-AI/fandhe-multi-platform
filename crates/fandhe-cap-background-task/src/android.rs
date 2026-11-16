//! Android 向けバックグラウンド処理実装（Issue #1046、親 #1042、macOS 実装 #1043 の
//! 横展開）。`WorkManager`/`OneTimeWorkRequest` を呼ぶ Java 側 `BackgroundTaskHelper` の
//! static 入口を橋渡しする。`driver.rs` の Android 経路から
//! [`AndroidBackgroundTask`] を使う（`crate::contract::BackgroundTask`
//! 実装）。
//!
//! # 記録を書く主体（macOS 実装との違い）
//!
//! macOS 実装（`macos.rs`）は OS の発火コールバックが Rust
//! プロセス内へ届くため、Rust 側が `write_record` で記録を書く。`WorkManager` は
//! Activity も cdylib も動いていない冷えたプロセスで `Worker` を起動しうるため、
//! `android_main` 前提の `fandhe_platform_android::android_app()` が `None` になる。この
//! プロセスで記録を書くと取りこぼすため、Java 側の `BackgroundTaskWorker`
//! （`platforms/android/app/src/main/java/com/fandhe/platform/BackgroundTaskWorker.java`）
//! が `crate::contract::RECORD_KEY`/`encode_record` と同じ形式・
//! 保存先（`getFilesDir()` 直下、[`super::android::data_dir`] と同じパス）へ直接
//! 書き込む。本モジュールは Rust プロセスが動いている間の `schedule`/`cancel`
//! の橋渡しと、保存済み記録の読み出し（[`read_record`]、ファイルは共通）だけを担う。
//!
//! # 二重予約は REPLACE
//!
//! 本 crate の `contract.rs` の契約（二重予約は置き換え）に合わせ、Java 側は
//! `ExistingWorkPolicy.REPLACE` で一意名 `enqueueUniqueWork` する
//! （`BackgroundTaskHelper.schedule` 参照）。
//!
//! # `RunResult::Expired` は使わない
//!
//! `Worker.doWork()` は即座に記録を書いて終わるため、OS の停止による中断の余地が
//! ほぼ無い。停止された `Worker` は `WorkManager` が再試行するだけで、結果は
//! 報告しない（`contract.rs` の `RunResult` doc 参照）。
//!
//! # 状態は参考値・`WorkManager` へは問い合わせない（オーナー承認済み方針、10/2、
//! Windows 実装 `windows.rs` と同じ形）
//!
//! 当初は `backgroundTaskStateBits()` で `WorkInfo` を毎回問い合わせていたが、
//! 次の 3 件の指摘（PR #1052）で破棄した。
//!
//! - `getWorkInfosForUniqueWork` が返す全履歴をビット合算するため、`REPLACE` で
//!   置き換えた旧世代の `FAILED`/`SUCCEEDED` が現在の予約状態に混ざる
//! - 背景スレッドでの照会完了が UI の再描画を起こさないため、初期値
//!   （`NotScheduled`）が再描画されるまで残り続ける
//! - 古いキャッシュが `schedule` 直後の `Scheduled` を上書きし、ポーリングが
//!   そのまま止まる
//!
//! [`AndroidBackgroundTask::state`] は [`WindowsBackgroundTask::state`]（Issue #1044）
//! と同じ形に単純化し、プロセス内に持つ直近の予約時刻（[`AndroidBackgroundTask::
//! scheduled_at_ms`]）と、保存済みの実行記録（[`read_record`]、共通 key）を比較
//! するだけで判定する: 予約時刻があり、記録がそれより古い（または無い）→
//! `Scheduled`、記録が予約時刻以降 → `Ran`、予約していない → `NotScheduled`
//! （記録があれば `Ran`）。取り消しで予約時刻を消す。この判定は参考値であり、
//! `WorkManager` 側の実際の状態との食い違いを正確に解消する契約は採らない
//! （アプリ再起動後は予約時刻が失われるため、再予約するまでは `NotScheduled` を
//! 返す。trait 契約で許容。OS 側の予約は残り、発火すれば記録される）。

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use crate::contract::ScheduleState;

#[cfg(target_os = "android")]
use crate::contract::{BackgroundTask, RunRecord, read_record};
#[cfg(target_os = "android")]
use crate::error::BackgroundTaskError;
#[cfg(target_os = "android")]
use fandhe_device::storage::Storage;
#[cfg(target_os = "android")]
use std::sync::{Arc, Mutex};

/// [`AndroidBackgroundTask::state`] の判定本体（ホストの `cargo test` で固定する
/// 純関数、モジュール doc「状態は参考値」参照）。`ran_at_unix_ms` は
/// [`read_record`] の読み出し失敗を「記録なし」に畳んだ後の値（`last_run()` が
/// 別途 `Err` を返す契約は保つ）。
fn decide_state(scheduled_at_ms: Option<u64>, ran_at_unix_ms: Option<u64>) -> ScheduleState {
    match (scheduled_at_ms, ran_at_unix_ms) {
        (Some(scheduled), Some(ran)) if ran >= scheduled => ScheduleState::Ran,
        (Some(_), _) => ScheduleState::Scheduled,
        // このプロセスで予約していない（再起動後・取り消し後）なら、過去の記録が
        // あっても `NotScheduled`（`Ran` は「直近の予約が実行済み」の意味。過去の
        // 実行結果は `last_run()` が表示する。PR #1052 codex 指摘）。
        (None, _) => ScheduleState::NotScheduled,
    }
}

/// `RunRecord` の保存先ファイル名（`fandhe_device::storage` の非公開
/// `encode_key(RECORD_KEY)` を 16 進符号化した結果の固定値）。本モジュールの
/// ホストテストが「Java 側が書く契約のファイルと Rust 側の読み出しが同じパスを
/// 指す」ことを固定するためだけに使う（`storage::encode_key` は非公開のため値を
/// 複製する。`crate::contract::RECORD_KEY` が変わればここも追随
/// が必要）。
#[cfg(test)]
const RECORD_FILE_NAME: &str = "6261636b67726f756e642d7461736b2e6c6173742d72756e";

/// 現在時刻（UNIX エポックからのミリ秒）。取得に失敗した場合は `0`
/// （`windows.rs::now_unix_ms` と同じ方針。`schedule` の正しさが
/// 依存する唯一の性質は「現在時刻の近似」であり、`0` へのフォールバックは安全側
/// に倒れる: 以後の `state()` 判定で「記録が常に予約時刻以降」に見えるだけで、
/// 失敗や嘘の成功にはならない）。
#[cfg(target_os = "android")]
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Android 実バックグラウンド処理実装（`WorkManager`、Issue #1046）。
/// `driver.rs::platform_background_task` の Android 分岐から構築される。
#[cfg(target_os = "android")]
pub struct AndroidBackgroundTask {
    /// このプロセスが最後に出した予約の時刻（`state()` が実行記録と比較する
    /// 参考値。モジュール doc「状態は参考値」参照）。
    scheduled_at_ms: Mutex<Option<u64>>,
    storage: Arc<dyn Storage>,
}

#[cfg(target_os = "android")]
impl AndroidBackgroundTask {
    /// `storage` は [`super::android::data_dir`] から作った `FsStorage` を渡す契約
    /// （`last_run()` の読み出し先が Java 側 `BackgroundTaskWorker` の書き込み先と
    /// 一致する、モジュール doc「記録を書く主体」参照）。
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self {
            scheduled_at_ms: Mutex::new(None),
            storage,
        }
    }

    /// Java 側 `BackgroundTaskHelper.schedule(Context)` を static 呼び出しする（戻り値 0 = 成功、それ以外 = 失敗）。
    /// JavaVM と Activity の取得は `with_activity_class` が持つため、ここに `unsafe` は無い。
    fn call_schedule(&self) -> bool {
        Self::call_static(jni::jni_str!("schedule"))
    }

    /// Java 側 `BackgroundTaskHelper.cancel(Context)` を static 呼び出しする（[`Self::call_schedule`] と同型）。
    fn call_cancel(&self) -> bool {
        Self::call_static(jni::jni_str!("cancel"))
    }

    fn call_static(method: &jni::strings::JNIStr) -> bool {
        use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
        use jni::objects::JValue;
        let Some(app) = android_app() else {
            return false;
        };
        let result = with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.BackgroundTaskHelper"),
            |env, activity, class| -> Result<i32, JniCallFailed> {
                let value = env.call_static_method(
                    class,
                    method,
                    jni::jni_sig!((android.content.Context) -> int),
                    &[JValue::Object(activity)],
                )?;
                Ok(value.i()?)
            },
        );
        matches!(result, Ok(0))
    }
}

#[cfg(target_os = "android")]
impl BackgroundTask for AndroidBackgroundTask {
    fn schedule(&self) -> Result<(), BackgroundTaskError> {
        // 予約時刻は enqueue より前に取る。制約の無い `OneTimeWorkRequest` は
        // enqueue の完了待ち中に実行され得るため、後で取ると実行記録の方が古く
        // 見え、実行済みでも `Scheduled` のまま残る（PR #1052 codex 指摘）。
        let scheduled_at = now_unix_ms();
        let ok = self.call_schedule();
        if ok {
            // `TASK_CREATE_OR_UPDATE` に相当する `REPLACE` 置き換えは Java 側で
            // 原子的に行われるため、置き換え前の旧予約時刻は残さない（二重予約は
            // 置き換え契約）。
            *self
                .scheduled_at_ms
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(scheduled_at);
            Ok(())
        } else {
            // JNI 呼び出し自体の失敗時は旧予約時刻（あれば）をそのまま残す
            // （`device-bridge-contracts.md`「9. 失敗時のリソース解放・共有状態
            // 復元」。嘘の成功を返さない）。
            Err(BackgroundTaskError::Internal("background task schedule"))
        }
    }

    fn cancel(&self) -> Result<(), BackgroundTaskError> {
        let ok = self.call_cancel();
        // 未予約での取り消しもべき等に `Ok`（`cancelUniqueWork` は未予約でも成功する
        // 契約、モジュール doc 参照）。JNI 呼び出し自体の失敗（`android_app()` が
        // `None`・メソッド未検出等）のみ `Err` にする。
        if ok {
            *self
                .scheduled_at_ms
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            Ok(())
        } else {
            Err(BackgroundTaskError::Internal("background task cancel"))
        }
    }

    fn state(&self) -> Result<ScheduleState, BackgroundTaskError> {
        let scheduled_at_ms = *self
            .scheduled_at_ms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // 読み出し失敗は「記録なし」側に倒す（`windows.rs::state`
        // と同じ方針。状態は参考値であり、`last_run()` が別途 `Err` を返す契約を
        // 保つ)。
        let ran_at_unix_ms = read_record(self.storage.as_ref())
            .ok()
            .flatten()
            .map(|record| record.ran_at_unix_ms);
        Ok(decide_state(scheduled_at_ms, ran_at_unix_ms))
    }

    fn last_run(&self) -> Result<Option<RunRecord>, BackgroundTaskError> {
        read_record(self.storage.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_state_scheduled_without_record_is_scheduled() {
        assert_eq!(decide_state(Some(100), None), ScheduleState::Scheduled);
    }

    #[test]
    fn decide_state_scheduled_with_older_record_is_scheduled() {
        assert_eq!(decide_state(Some(100), Some(50)), ScheduleState::Scheduled);
    }

    #[test]
    fn decide_state_record_at_or_after_schedule_is_ran() {
        assert_eq!(decide_state(Some(100), Some(100)), ScheduleState::Ran);
        assert_eq!(decide_state(Some(100), Some(150)), ScheduleState::Ran);
    }

    #[test]
    fn decide_state_cancelled_is_not_scheduled() {
        // 取り消しで `scheduled_at_ms` を消した後（`cancel()` 参照）、記録も無ければ
        // 未予約。
        assert_eq!(decide_state(None, None), ScheduleState::NotScheduled);
    }

    #[test]
    fn decide_state_unscheduled_with_record_is_not_scheduled() {
        assert_eq!(decide_state(None, Some(50)), ScheduleState::NotScheduled);
    }

    /// 保存名の固定: `FsStorage` に `write_record` で書いたファイルが
    /// [`RECORD_FILE_NAME`] と一致することを確かめる。Java 側 `BackgroundTaskWorker`
    /// はこの固定値をハードコードして同じパスへ書く契約（モジュール doc「記録を
    /// 書く主体」参照）。
    #[test]
    fn record_file_name_matches_fs_storage_encoding() {
        use crate::contract::{RunRecord, RunResult, write_record};
        use fandhe_device::DataDir;
        use fandhe_device::storage::FsStorage;

        let dir = std::env::temp_dir().join(format!(
            "fandhe-background-task-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = FsStorage::new(DataDir(dir.clone()));
        write_record(
            &storage,
            &RunRecord {
                ran_at_unix_ms: 1,
                result: RunResult::Completed,
            },
        )
        .expect("write は成功する");
        assert!(dir.join(RECORD_FILE_NAME).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
