//! iOS 向け歩数実装（Issue #1248、契約と dispatch は #1247）。
//! `device/pedometer.rs::implementation` の iOS 経路から呼ばれ、`ios/fandhe_ios_pedometer.m`
//! （`CMPedometer.queryPedometerData`）を C ABI 経由で呼ぶ。ワーカースレッドでブロッキングに
//! 1 回取得し、semaphore で `READ_TIMEOUT` まで待つ（継続 push は使わない）。
//!
//! 契約表との対応: 失敗・非対応・拒否・未取得は 0 歩へ畳まず [`PedometerFailure`] へ写像
//! する（6）。期限は引数だけが決める（3）。`CMPedometer` は関数を抜けると解放され、残る
//! listener は無い（9）。再描画: OS の handler は semaphore で同じワーカーへ結果を戻し、
//! ワーカーが `CompletionSender::send`（直後に `ctx.request_repaint()`）で画面へ入れる。
//! OS コールバックが egui の外で共有状態を書き換えることは無いため、
//! `crate::ios::request_repaint` の明示呼び出しは要らない（4）。
//!
//! FFI / 本体は `cfg(target_os = "ios")` に閉じ、純関数 `result_from_code` はホストで
//! テストできる（`device.rs` の `cfg(any(ios, test))` はそのため）。
//! 既知の天井: 初回の許可ダイアログに期限内に答えないと `NoReading`。シミュレータは常に非対応。

use std::sync::Arc;

#[cfg(target_os = "ios")]
use std::time::Duration;

#[cfg(not(target_os = "ios"))]
use fandhe_device::pedometer::UnsupportedPedometer;
use fandhe_device::pedometer::{total_steps, Pedometer, PedometerFailure};

/// `.m` の戻りコード（0 = OK / 1 = 非対応 / 2 = 権限拒否 / 3 = 未取得 / 4 = 失敗）を
/// 契約上の結果へ写像する唯一の箇所。未知の値は `Failed`。値域検査は `total_steps` に任せる。
fn result_from_code(code: i32, steps: i64) -> Result<u64, PedometerFailure> {
    match code {
        0 => total_steps(std::iter::once(steps)),
        1 => Err(PedometerFailure::Unsupported),
        2 => Err(PedometerFailure::PermissionDenied),
        3 => Err(PedometerFailure::NoReading),
        _ => Err(PedometerFailure::Failed),
    }
}

// SAFETY: `ios/fandhe_ios_pedometer.m` が定義する C ABI（`build.rs` が iOS ターゲットのみ・
// feature `device` 有効時のみコンパイル・リンクする）。契約は同 .m 冒頭コメント。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_pedometer_read(timeout_ms: u64, out_steps: *mut i64) -> i32;
}

/// `CMPedometer` による `Pedometer` 実装。
#[cfg(target_os = "ios")]
struct IosPedometer;

#[cfg(target_os = "ios")]
impl Pedometer for IosPedometer {
    #[allow(unsafe_code)]
    fn read_steps(&self, timeout: Duration) -> Result<u64, PedometerFailure> {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let mut steps: i64 = 0;
        // SAFETY: `steps` はこの呼び出しの間だけ有効なスタック変数。`.m` は戻る前に
        // 呼び出しスレッド上でのみ書き込み、遅れて来る handler は自身の `__block` 領域にしか
        // 触れない。戻りコードが 0 以外のとき `steps` は読まれない。
        let code = unsafe { fandhe_ios_pedometer_read(timeout_ms, &mut steps) };
        result_from_code(code, steps)
    }
}

/// iOS の `Pedometer` 実装を返す。ホストの test ビルドでは非対応を返す。
pub(crate) fn implementation() -> Arc<dyn Pedometer> {
    #[cfg(target_os = "ios")]
    {
        Arc::new(IosPedometer)
    }
    #[cfg(not(target_os = "ios"))]
    {
        Arc::new(UnsupportedPedometer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::pedometer::READ_TIMEOUT;

    #[test]
    fn failure_codes_never_become_success() {
        assert_eq!(result_from_code(1, 0), Err(PedometerFailure::Unsupported));
        assert_eq!(
            result_from_code(2, 0),
            Err(PedometerFailure::PermissionDenied)
        );
        assert_eq!(result_from_code(3, 0), Err(PedometerFailure::NoReading));
        assert_eq!(result_from_code(4, 0), Err(PedometerFailure::Failed));
        assert_eq!(result_from_code(99, 5), Err(PedometerFailure::Failed));
        assert_eq!(result_from_code(-1, 5), Err(PedometerFailure::Failed));
    }

    #[test]
    fn ok_code_maps_steps() {
        assert_eq!(result_from_code(0, 0), Ok(0));
        assert_eq!(result_from_code(0, 1234), Ok(1234));
        assert_eq!(result_from_code(0, -1), Err(PedometerFailure::Failed));
    }

    // iOS では implementation() が実体で CoreMotion を読むため、非 iOS ホストに限定する。
    #[cfg(not(target_os = "ios"))]
    #[test]
    fn host_implementation_is_unsupported() {
        assert_eq!(
            implementation().read_steps(READ_TIMEOUT),
            Err(PedometerFailure::Unsupported)
        );
    }
}
