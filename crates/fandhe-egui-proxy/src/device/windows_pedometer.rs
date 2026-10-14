//! Windows 向け歩数実装（Issue #1247）。`device/pedometer.rs` の Windows 経路から呼ばれ、
//! WinRT `Windows.Devices.Sensors.Pedometer` から累積歩数を 1 回読む（`Pedometer` トレイトを
//! 実装。呼び出し側がワーカースレッドで呼ぶ前提）。
//!
//! # 流れ
//!
//! 1. `Pedometer::GetDefaultAsync()` を呼び、`SetCompleted` ハンドラから `mpsc` に結果を送る
//!    （`windows_biometric.rs` と同じ書き方）
//! 2. `recv_timeout(timeout)` で待つ。期限切れなら `Cancel()` して `NoReading`
//! 3. 結果を写像する。null（`e.code().0 == 0`、`windows_haptics.rs` と同じ判定）→
//!    `Unsupported`、`E_ACCESSDENIED` → `PermissionDenied`、それ以外 → `Failed`
//! 4. `GetCurrentReadings()` の各 `PedometerReading::CumulativeSteps()` を
//!    `total_steps` で合算する（空は `NoReading`、負値は `Failed`、0 歩は成功）
//!
//! HRESULT・歩数はログにも画面にも出さない（security.md A09）。`unsafe` と `RoInitialize` は
//! 使わない（既存の Windows 実装群と同じ前提）。期限は引数 `timeout` のみ（類型 3）。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.pedometer>
//!
//! # 既知の天井
//!
//! - パッケージ無しの素の exe から届くか、設定の「モーション」プライバシーで拒否されたときの
//!   HRESULT は実機未確認（#1244）。届かないと判明したら対応表の `done` を取り下げる
//! - `GetCurrentReadings` は Windows 10 1607 以降。それ未満はメソッド呼び出し失敗として
//!   `Failed` になる
//! - ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use fandhe_device::pedometer::{total_steps, Pedometer, PedometerFailure};
use windows::Devices::Sensors::Pedometer as WinPedometer;
use windows_future::{AsyncOperationCompletedHandler, AsyncStatus};

/// `E_ACCESSDENIED`（0x80070005）。権限拒否の写像に使う。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

/// `GetDefaultAsync` の失敗 HRESULT を契約の失敗種別へ写す（ホストでも検証できる純関数）。
/// 0（S_OK + null）は搭載センサー無し。
fn failure_from_hresult(code: i32) -> PedometerFailure {
    match code {
        0 => PedometerFailure::Unsupported,
        E_ACCESSDENIED => PedometerFailure::PermissionDenied,
        _ => PedometerFailure::Failed,
    }
}

/// WinRT のエラーを共通写像へ通す（同期失敗・`GetCurrentReadings` 失敗を含む全呼び出し箇所用）。
fn failure_from_error(e: &windows::core::Error) -> PedometerFailure {
    failure_from_hresult(e.code().0)
}

/// 非同期操作の終了状態から失敗 HRESULT を決める。`Error` のときは `GetResults()` が返す
/// 元の HRESULT を保つ（`-1` に潰すと `E_ACCESSDENIED` が `Failed` になる）。
/// `results_code` は `GetResults()` が失敗したときの HRESULT。
fn async_failure_code(status: AsyncStatus, results_code: Option<i32>) -> i32 {
    match (status, results_code) {
        (AsyncStatus::Error, Some(code)) => code,
        _ => -1,
    }
}

/// `Pedometer` センサーから累積歩数を読む実装。
pub(crate) struct WindowsPedometer;

/// プロセスで共有する実装を返す（`device/pedometer.rs::implementation` から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn Pedometer> {
    Arc::new(WindowsPedometer)
}

impl Pedometer for WindowsPedometer {
    fn read_steps(&self, timeout: Duration) -> Result<u64, PedometerFailure> {
        let op = WinPedometer::GetDefaultAsync().map_err(|e| failure_from_error(&e))?;
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
        .map_err(|e| failure_from_error(&e))?;

        let sensor = match rx.recv_timeout(timeout) {
            Ok(Ok(sensor)) => sensor,
            Ok(Err(code)) => return Err(failure_from_hresult(code)),
            Err(_) => {
                // 期限切れ: 保留中の操作を取り消してから戻る。
                let _ = op.Cancel();
                return Err(PedometerFailure::NoReading);
            }
        };

        let readings = sensor
            .GetCurrentReadings()
            .map_err(|e| failure_from_error(&e))?;
        let mut steps = Vec::new();
        for pair in readings {
            let reading = pair.Value().map_err(|e| failure_from_error(&e))?;
            steps.push(i64::from(
                reading
                    .CumulativeSteps()
                    .map_err(|e| failure_from_error(&e))?,
            ));
        }
        total_steps(steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hresult_mapping_never_yields_success() {
        assert_eq!(failure_from_hresult(0), PedometerFailure::Unsupported);
        assert_eq!(
            failure_from_hresult(E_ACCESSDENIED),
            PedometerFailure::PermissionDenied
        );
        assert_eq!(failure_from_hresult(-1), PedometerFailure::Failed);
    }

    #[test]
    fn async_error_keeps_access_denied() {
        let code = async_failure_code(AsyncStatus::Error, Some(E_ACCESSDENIED));
        assert_eq!(
            failure_from_hresult(code),
            PedometerFailure::PermissionDenied
        );
        assert_eq!(async_failure_code(AsyncStatus::Canceled, None), -1);
        assert_eq!(async_failure_code(AsyncStatus::Error, None), -1);
    }

    #[test]
    fn sync_errors_map_access_denied() {
        // GetDefaultAsync・GetCurrentReadings・各 reading 取得の同期失敗が共通写像を通る。
        let e = windows::core::Error::from(windows::core::HRESULT(E_ACCESSDENIED));
        assert_eq!(failure_from_error(&e), PedometerFailure::PermissionDenied);
    }
}
