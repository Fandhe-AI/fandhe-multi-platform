//! Windows 向け在席検知実装（Issue #960）。`device/human_presence.rs` の Windows 経路から呼ばれ、
//! WinRT `Windows.Devices.Sensors.HumanPresenceSensor` から現在の在席を 1 回読む
//! （`HumanPresence` トレイトを実装。呼び出し側がワーカースレッドで呼ぶ前提）。
//!
//! # 流れ
//!
//! 1. `HumanPresenceSensor::GetDefaultAsync()` を呼び、`SetCompleted` ハンドラから `mpsc` に結果を送る
//!    （`windows_pedometer.rs` と同じ書き方）
//! 2. `recv_timeout(timeout)` で待つ。期限切れなら `Cancel()` して `NoReading`
//! 3. センサーの null（`e.code().0 == 0`）→ `Unsupported`。`GetCurrentReading()` の null → `NoReading`
//!    （センサー側と null の意味が違う）。`E_ACCESSDENIED` → `PermissionDenied`、
//!    `REGDB_E_CLASSNOTREG` → `Unsupported`、それ以外 → `Failed`（`failure_from_hresult` に集約）
//! 4. `Presence()` の値を `presence_from_raw` へ通す（列挙外は `Unknown`）
//!
//! `ReadingChanged` は購読せず、センサーと reading は関数内で drop する。HRESULT・在席の値は
//! ログにも画面にも出さない（security.md A09）。`unsafe` と `RoInitialize` は使わない。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.humanpresencesensor>
//!
//! # 既知の天井
//!
//! - 正常系・拒否時の HRESULT・クラス未登録時の HRESULT・購読なしの `GetCurrentReading` が値を
//!   返すかは実機未確認（#954）。購読なしで常に null と判明したら対応表の `done` を取り下げ、
//!   購読方式で起案し直す
//! - ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use fandhe_device::human_presence::{
    presence_from_raw, HumanPresence, HumanPresenceFailure, Presence,
};
use windows::Devices::Sensors::HumanPresenceSensor;
use windows_future::{AsyncOperationCompletedHandler, AsyncStatus};

/// `E_ACCESSDENIED`（0x80070005）。権限拒否の写像に使う。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
/// `REGDB_E_CLASSNOTREG`（0x80040154）。この OS にクラスが無い（非対応）の写像に使う。
const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154_u32 as i32;

/// 失敗 HRESULT を契約の失敗種別へ写す（ホストでも検証できる純関数）。
/// 0（S_OK + null）は呼び出し箇所ごとに意味が違うため `on_null` で受ける。
fn failure_from_hresult(code: i32, on_null: HumanPresenceFailure) -> HumanPresenceFailure {
    match code {
        0 => on_null,
        E_ACCESSDENIED => HumanPresenceFailure::PermissionDenied,
        REGDB_E_CLASSNOTREG => HumanPresenceFailure::Unsupported,
        _ => HumanPresenceFailure::Failed,
    }
}

/// WinRT のエラーを共通写像へ通す（全呼び出し箇所用）。
fn failure_from_error(
    e: &windows::core::Error,
    on_null: HumanPresenceFailure,
) -> HumanPresenceFailure {
    failure_from_hresult(e.code().0, on_null)
}

/// 非同期操作の終了状態から失敗 HRESULT を決める。`Error` のときは `GetResults()` が返す
/// 元の HRESULT を保つ（`-1` に潰すと `E_ACCESSDENIED` が `Failed` になる）。
fn async_failure_code(status: AsyncStatus, results_code: Option<i32>) -> i32 {
    match (status, results_code) {
        (AsyncStatus::Error, Some(code)) => code,
        _ => -1,
    }
}

/// `HumanPresenceSensor` から在席を読む実装。
pub(crate) struct WindowsHumanPresence;

/// プロセスで共有する実装を返す（`device/human_presence.rs::implementation` から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn HumanPresence> {
    Arc::new(WindowsHumanPresence)
}

impl HumanPresence for WindowsHumanPresence {
    fn read_presence(&self, timeout: Duration) -> Result<Presence, HumanPresenceFailure> {
        use HumanPresenceFailure::{NoReading, Unsupported};
        let op = HumanPresenceSensor::GetDefaultAsync()
            .map_err(|e| failure_from_error(&e, Unsupported))?;
        let (tx, rx) = mpsc::channel();
        op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
            let result = match (op.ok(), status) {
                (Ok(op), AsyncStatus::Completed) => op.GetResults().map_err(|e| e.code().0),
                // エラーは元の HRESULT を取り出す。キャンセル等は -1（偽の成功を作らない）。
                (Ok(op), AsyncStatus::Error) => Err(async_failure_code(
                    status,
                    op.GetResults().err().map(|e| e.code().0),
                )),
                _ => Err(async_failure_code(status, None)),
            };
            // 受信側が期限切れで落ちていても無視してよい。
            let _ = tx.send(result);
            Ok(())
        }))
        .map_err(|e| failure_from_error(&e, Unsupported))?;

        let sensor = match rx.recv_timeout(timeout) {
            Ok(Ok(sensor)) => sensor,
            Ok(Err(code)) => return Err(failure_from_hresult(code, Unsupported)),
            Err(_) => {
                // 期限切れ: 保留中の操作を取り消してから戻る。
                let _ = op.Cancel();
                return Err(NoReading);
            }
        };

        let reading = sensor
            .GetCurrentReading()
            .map_err(|e| failure_from_error(&e, NoReading))?;
        let raw = reading
            .Presence()
            .map_err(|e| failure_from_error(&e, NoReading))?;
        Ok(presence_from_raw(raw.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HumanPresenceFailure::*;

    #[test]
    fn hresult_mapping_never_yields_success() {
        assert_eq!(failure_from_hresult(0, Unsupported), Unsupported);
        assert_eq!(failure_from_hresult(0, NoReading), NoReading);
        assert_eq!(
            failure_from_hresult(E_ACCESSDENIED, Unsupported),
            PermissionDenied
        );
        assert_eq!(
            failure_from_hresult(REGDB_E_CLASSNOTREG, Failed),
            Unsupported
        );
        assert_eq!(failure_from_hresult(-1, Unsupported), Failed);
    }

    #[test]
    fn async_error_keeps_access_denied_and_cancel_is_failed() {
        let code = async_failure_code(AsyncStatus::Error, Some(E_ACCESSDENIED));
        assert_eq!(failure_from_hresult(code, Unsupported), PermissionDenied);
        let canceled = async_failure_code(AsyncStatus::Canceled, None);
        assert_eq!(canceled, -1);
        assert_eq!(failure_from_hresult(canceled, Unsupported), Failed);
    }

    #[test]
    fn sync_errors_map_access_denied() {
        let e = windows::core::Error::from(windows::core::HRESULT(E_ACCESSDENIED));
        assert_eq!(failure_from_error(&e, NoReading), PermissionDenied);
    }
}
