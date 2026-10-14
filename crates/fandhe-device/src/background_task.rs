//! バックグラウンド処理の抽象（Phase 11 の拡充ループ、親 #1042、Issue #1043）。
//!
//! 扱う範囲は「処理を 1 件予約する → OS の都合で最大 1 回実行される → 実行時刻と
//! 結果を保存する → 次に画面を開いたとき表示する」の 1 本道に限る。繰り返し実行・
//! 長時間処理・実行時刻の保証は扱わない（対応表 `docs/device/capabilities/
//! background-task.json` のトップレベル `note` 参照）。
//!
//! # 契約（各実装・呼び出し元が守る不変条件）
//!
//! 設計判断として、予約・取り消し・
//! 実行の順序が入れ替わる競合条件を厳密に処理する契約は採らず、次の簡略版で統一する。
//! 守るのは 1 プロセス内の順序と嘘の成功の禁止で、複数タブ・複数プロセス・同一
//! ミリ秒の重なりは既知の制約とする（`AGENTS.md`「端末機能の並行性は守備範囲で
//! 判定する」）。
//!
//! - **予約できない場合**: 設定で無効・権限が未宣言・非対応では `Err`
//!   （`Disabled`/`PermissionDenied`/`Unsupported`/`Internal`）を返す。予約の成功を
//!   装わない（`device-bridge-contracts.md`「6. 嘘の成功」）。初期化に失敗した実装は
//!   [`UnsupportedBackgroundTask`] へ倒し、ダミーの成功実装は使わない
//! - **実行時刻は保証しない**: `Scheduled` のまま実行されないことは正常で、失敗
//!   として扱わない（契約表「10. キャンセル・正常終了・失敗の三値」）。本クレートは
//!   期限を設けない（契約表「3. 期限・キャンセル」は OS 側・呼び出し元の責務）
//! - **二重予約は置き換え**: [`BackgroundTask::schedule`] は常に高々 1 件の予約に
//!   する。2 回目の呼び出しは最後の予約だけを有効にし、戻り値は `Ok(())`
//!   （iOS の同一 identifier 再 submit・Android の `ExistingWorkPolicy.REPLACE`・
//!   Windows の `TASK_CREATE_OR_UPDATE` と揃う意味論）
//! - **取り消しはベストエフォート**: [`BackgroundTask::cancel`] は予約が無くても
//!   常に `Ok(())`（べき等）。取り消し要求の直前に実行が始まっていた場合、その
//!   実行結果が [`BackgroundTask::last_run`] に残ることがある（取り消しと実行の
//!   順序が入れ替わっても正確に防ぐ契約は採らない）
//! - **実行記録は「時刻の新しい方を残す」**: 複数の実行（二重予約の置き換え前後・
//!   取り消し直後の発火等）が競合して記録を書くとき、各実装は厳密な世代照合を
//!   行わず、既存の記録より古い `ran_at_unix_ms` では上書きしない程度で十分とする
//!   （[`write_record_if_newer`] 参照）
//! - **[`BackgroundTask::state`] は参考値**: OS への問い合わせ結果、または実装が
//!   自前で持つ「予約した」という印をそのまま返してよい。プロセス再起動後や
//!   OS 側の実際の状態との食い違いを正確に解消する契約は採らない
//! - **[`BackgroundTask::last_run`] の 3 値**
//!   - `Ok(None)`: 未実行
//!   - `Ok(Some(_))`: 実行済み
//!   - `Err(_)`: 読み出しに失敗（未実行と混同しない。`device-bridge-contracts.md`
//!     「6. 嘘の成功」）
//! - **[`ScheduleState::Ran`] の意味**: 直近の予約が実行済みであることの参考値。
//!   プロセス再起動後や取り消し後は `NotScheduled` を返してよい。実行結果そのものは
//!   [`BackgroundTask::last_run`] が永続化する

use crate::DeviceError;

/// 予約状態（[`BackgroundTask::state`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleState {
    /// 予約していない（初期状態・取り消し後・プロセス再起動後）。
    NotScheduled,
    /// 予約済みで、まだ実行されていない（実行時刻は保証しない）。
    Scheduled,
    /// このプロセスが出した予約が実行済み。
    Ran,
}

/// 実行結果（[`RunRecord::result`]）。`Expired` は iOS の `expirationHandler`等、
/// OS の実行可能時間切れを検知できる実装が使う想定。macOS 実装は `Completed` のみを
/// 書き込む。Android（`WorkManager`、Issue #1046）の `Worker.doWork()` は即座に
/// 記録を書いて終わるため停止による中断の余地がほぼ無く、同様に `Completed` のみを
/// 書き込む（`device/android_background_task.rs` モジュール doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResult {
    /// 処理が完了した。
    Completed,
    /// OS の実行可能時間切れで中断された。
    Expired,
}

/// 保存された実行記録（[`BackgroundTask::last_run`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunRecord {
    /// 実行時刻（UNIX エポックからのミリ秒）。
    pub ran_at_unix_ms: u64,
    /// 実行結果。
    pub result: RunResult,
}

/// バックグラウンド処理の契約（モジュール doc の「契約」節参照）。
pub trait BackgroundTask: Send + Sync {
    /// 処理を 1 件予約する。既に予約済みなら置き換える（二重予約は置き換え契約）。
    fn schedule(&self) -> Result<(), DeviceError>;
    /// 予約を取り消す。未予約でも `Ok(())`（べき等契約）。
    fn cancel(&self) -> Result<(), DeviceError>;
    /// 現在の予約状態を返す。
    fn state(&self) -> Result<ScheduleState, DeviceError>;
    /// 最後に保存された実行記録を返す（3 値の契約はモジュール doc 参照）。
    fn last_run(&self) -> Result<Option<RunRecord>, DeviceError>;
}

/// 対応不可プラットフォーム用（初期化失敗時のフォールバックも含む）。常に
/// `Unsupported("background task")` を返す（ダミーの成功実装へは倒さない契約）。
pub struct UnsupportedBackgroundTask;

impl BackgroundTask for UnsupportedBackgroundTask {
    fn schedule(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("background task"))
    }

    fn cancel(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("background task"))
    }

    fn state(&self) -> Result<ScheduleState, DeviceError> {
        Err(DeviceError::Unsupported("background task"))
    }

    fn last_run(&self) -> Result<Option<RunRecord>, DeviceError> {
        Err(DeviceError::Unsupported("background task"))
    }
}

/// [`RunRecord`] の保存先 key（全 OS・Web 共通の SSOT）。`storage::validate_key` の
/// 許可文字集合（英数字・`.`・`_`・`-`）のみで作る。
pub const RECORD_KEY: &str = "background-task.last-run";

/// 保存形式の version（[`decode_record`] が未知の値を拒否する）。
const RECORD_VERSION: u64 = 1;

fn result_tag(result: RunResult) -> &'static str {
    match result {
        RunResult::Completed => "completed",
        RunResult::Expired => "expired",
    }
}

/// [`RunRecord`] を保存形式（UTF-8 JSON）へ符号化する純関数。現在時刻は呼び出し側が
/// 渡す契約のため、本関数自体は時刻取得を行わない（モジュール doc 参照。
/// `SystemTime::now()` は wasm32-unknown-unknown で panic し、本モジュールは
/// `fandhe-web-dom` からもコンパイルされるため）。
pub fn encode_record(record: &RunRecord) -> Vec<u8> {
    serde_json::json!({
        "version": RECORD_VERSION,
        "ranAtUnixMs": record.ran_at_unix_ms,
        "result": result_tag(record.result),
    })
    .to_string()
    .into_bytes()
}

/// [`encode_record`] の逆変換。未知の `version`・未知の `result`・型不一致はすべて
/// `Err(DeviceError::Internal("background task record"))` にする（壊れた値を
/// 「未実行」として成功へ畳まない、モジュール doc「last_run の 3 値」参照）。
pub fn decode_record(bytes: &[u8]) -> Result<RunRecord, DeviceError> {
    const ERR: DeviceError = DeviceError::Internal("background task record");

    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ERR)?;
    let version = value.get("version").and_then(serde_json::Value::as_u64);
    if version != Some(RECORD_VERSION) {
        return Err(ERR);
    }
    let ran_at_unix_ms = value
        .get("ranAtUnixMs")
        .and_then(serde_json::Value::as_u64)
        .ok_or(ERR)?;
    let result = match value.get("result").and_then(serde_json::Value::as_str) {
        Some("completed") => RunResult::Completed,
        Some("expired") => RunResult::Expired,
        _ => return Err(ERR),
    };
    Ok(RunRecord {
        ran_at_unix_ms,
        result,
    })
}

/// [`Storage`](crate::storage::Storage) 経由で [`RunRecord`] を保存する薄い補助関数
/// （各 OS 実装が共用する、モジュール doc 参照）。
pub fn write_record(
    storage: &dyn crate::storage::Storage,
    record: &RunRecord,
) -> Result<(), DeviceError> {
    storage.write(RECORD_KEY, &encode_record(record))
}

/// [`Storage`](crate::storage::Storage) 経由で [`RunRecord`] を読み出す薄い補助関数。
/// キーが無ければ `Ok(None)`（未実行）、読み出し・形式解釈に失敗すれば `Err`
/// （未実行と混同しない、モジュール doc「last_run の 3 値」参照）。
pub fn read_record(
    storage: &dyn crate::storage::Storage,
) -> Result<Option<RunRecord>, DeviceError> {
    match storage.read(RECORD_KEY)? {
        None => Ok(None),
        Some(bytes) => decode_record(&bytes).map(Some),
    }
}

/// [`write_record`] の緩和版。既存の記録より `ran_at_unix_ms` が古ければ書かずに
/// `Ok(())` を返す（モジュール doc「契約」節「実行記録は時刻の新しい方を残す」
/// 参照。複数の実行が競合して記録を書くとき、厳密な世代照合の代わりに使う薄い
/// 保護）。既存記録の形式が壊れている場合は上書きして回復する（壊れた記録を
/// 「新しい」とみなすと以後ずっと書けなくなるため）。保存層の読み出しエラーは
/// 既存記録を確認できないので書かずに `Err` を返す。
///
/// 比較と書き込みは不可分ではない。複数のプロセス（Windows のタスク起動プロセス等）
/// が同時に書くと、最後に書いた方が残る（古い時刻が残り得る）。1 件の予約が同時に
/// 複数回実行されることは通常なく、契約を緩めた判断（オーナー判断 10/2）に沿って
/// プロセス間ロックは持たない（PR #1049 で一度入れたロックが取得失敗時の扱い等の
/// 指摘を重ねたため撤去した経緯がある）。
pub fn write_record_if_newer(
    storage: &dyn crate::storage::Storage,
    record: &RunRecord,
) -> Result<(), DeviceError> {
    // 保存層の読み出しエラー（一時的な I/O 失敗等）は既存の新しい記録を確認できない
    // ため `Err` で返す。上書きして回復するのは、読めたが形式が壊れている場合だけ
    // （PR #1053 codex 指摘）。
    if let Some(bytes) = storage.read(RECORD_KEY)? {
        if let Ok(existing) = decode_record(&bytes) {
            if existing.ran_at_unix_ms > record.ran_at_unix_ms {
                return Ok(());
            }
        }
    }
    write_record(storage, record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use std::sync::Mutex;

    #[test]
    fn unsupported_background_task_returns_unsupported_for_all_methods() {
        let t = UnsupportedBackgroundTask;
        assert!(matches!(
            t.schedule(),
            Err(DeviceError::Unsupported("background task"))
        ));
        assert!(matches!(
            t.cancel(),
            Err(DeviceError::Unsupported("background task"))
        ));
        assert!(matches!(
            t.state(),
            Err(DeviceError::Unsupported("background task"))
        ));
        assert!(matches!(
            t.last_run(),
            Err(DeviceError::Unsupported("background task"))
        ));
    }

    #[test]
    fn record_key_passes_validate_key() {
        assert!(crate::storage::validate_key(RECORD_KEY).is_ok());
    }

    #[test]
    fn encode_decode_round_trips_completed() {
        let record = RunRecord {
            ran_at_unix_ms: 1_700_000_000_000,
            result: RunResult::Completed,
        };
        let decoded = decode_record(&encode_record(&record)).expect("decode は成功する");
        assert_eq!(decoded, record);
    }

    #[test]
    fn encode_decode_round_trips_expired() {
        let record = RunRecord {
            ran_at_unix_ms: 0,
            result: RunResult::Expired,
        };
        let decoded = decode_record(&encode_record(&record)).expect("decode は成功する");
        assert_eq!(decoded, record);
    }

    #[test]
    fn decode_rejects_invalid_json() {
        assert!(decode_record(b"not json").is_err());
    }

    #[test]
    fn decode_rejects_unknown_version() {
        let bytes = serde_json::json!({
            "version": 2,
            "ranAtUnixMs": 1,
            "result": "completed",
        })
        .to_string()
        .into_bytes();
        assert!(decode_record(&bytes).is_err());
    }

    #[test]
    fn decode_rejects_unknown_result() {
        let bytes = serde_json::json!({
            "version": 1,
            "ranAtUnixMs": 1,
            "result": "retrying",
        })
        .to_string()
        .into_bytes();
        assert!(decode_record(&bytes).is_err());
    }

    #[test]
    fn decode_rejects_non_numeric_ran_at() {
        let bytes = serde_json::json!({
            "version": 1,
            "ranAtUnixMs": "not a number",
            "result": "completed",
        })
        .to_string()
        .into_bytes();
        assert!(decode_record(&bytes).is_err());
    }

    /// インメモリ `Storage`（`fandhe-egui-proxy::device::tests::MemStorage` と同型）。
    #[derive(Default)]
    struct MemStorage(Mutex<Option<Vec<u8>>>);

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

    #[test]
    fn read_record_returns_none_when_key_absent() {
        let storage = MemStorage::default();
        assert_eq!(read_record(&storage).expect("read は成功する"), None);
    }

    #[test]
    fn read_record_round_trips_via_write_record() {
        let storage = MemStorage::default();
        let record = RunRecord {
            ran_at_unix_ms: 123,
            result: RunResult::Completed,
        };
        write_record(&storage, &record).expect("write は成功する");
        assert_eq!(
            read_record(&storage).expect("read は成功する"),
            Some(record)
        );
    }

    #[test]
    fn read_record_fails_on_corrupt_bytes() {
        let storage = MemStorage::default();
        storage
            .write(RECORD_KEY, b"corrupt")
            .expect("write は成功する");
        assert!(read_record(&storage).is_err());
    }

    #[test]
    fn read_record_propagates_storage_io_error() {
        let storage = ErrOnReadStorage;
        assert!(read_record(&storage).is_err());
    }

    #[test]
    fn write_record_if_newer_keeps_newer_existing_record() {
        let storage = MemStorage::default();
        let newer = RunRecord {
            ran_at_unix_ms: 2_000,
            result: RunResult::Completed,
        };
        write_record(&storage, &newer).expect("write は成功する");

        let older = RunRecord {
            ran_at_unix_ms: 1_000,
            result: RunResult::Expired,
        };
        write_record_if_newer(&storage, &older).expect("成功する（書かずに Ok）");
        assert_eq!(read_record(&storage).expect("read は成功する"), Some(newer));
    }

    #[test]
    fn write_record_if_newer_overwrites_older_existing_record() {
        let storage = MemStorage::default();
        let older = RunRecord {
            ran_at_unix_ms: 1_000,
            result: RunResult::Completed,
        };
        write_record(&storage, &older).expect("write は成功する");

        let newer = RunRecord {
            ran_at_unix_ms: 2_000,
            result: RunResult::Expired,
        };
        write_record_if_newer(&storage, &newer).expect("成功する");
        assert_eq!(read_record(&storage).expect("read は成功する"), Some(newer));
    }

    #[test]
    fn write_record_if_newer_overwrites_corrupt_record() {
        let storage = MemStorage::default();
        storage
            .write(RECORD_KEY, b"corrupt")
            .expect("write は成功する");
        let record = RunRecord {
            ran_at_unix_ms: 1,
            result: RunResult::Completed,
        };
        write_record_if_newer(&storage, &record).expect("壊れた記録は上書きして回復する");
        assert_eq!(
            read_record(&storage).expect("read は成功する"),
            Some(record)
        );
    }

    #[test]
    fn write_record_if_newer_returns_err_on_read_io_error() {
        let record = RunRecord {
            ran_at_unix_ms: 1,
            result: RunResult::Completed,
        };
        // 読み出しだけ失敗し書き込みは成功する保存層（一時的な I/O 失敗）。
        struct ReadErrWriteOk(Mutex<bool>);
        impl Storage for ReadErrWriteOk {
            fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
                Err(DeviceError::InvalidKey(key.to_string()))
            }
            fn write(&self, _key: &str, _bytes: &[u8]) -> Result<(), DeviceError> {
                *self.0.lock().expect("lock は成功する") = true;
                Ok(())
            }
        }
        let storage = ReadErrWriteOk(Mutex::new(false));
        assert!(write_record_if_newer(&storage, &record).is_err());
        assert!(
            !*storage.0.lock().expect("lock は成功する"),
            "書き込んではいけない"
        );
    }

    #[test]
    fn write_record_if_newer_writes_when_no_existing_record() {
        let storage = MemStorage::default();
        let record = RunRecord {
            ran_at_unix_ms: 1_000,
            result: RunResult::Completed,
        };
        write_record_if_newer(&storage, &record).expect("成功する");
        assert_eq!(
            read_record(&storage).expect("read は成功する"),
            Some(record)
        );
    }
}
