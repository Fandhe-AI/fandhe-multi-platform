//! iOS 向けヘルスデータ（歩数）実装（Issue #1090、契約と dispatch は #1089、親 #149）。
//! `device/health_data.rs` の iOS 経路から呼ばれ、`ios/fandhe_ios_health_data.m`（HealthKit の
//! `HKStatisticsQuery`）を C ABI 経由で呼ぶ。契約の正は `fandhe_device::health_data`。
//! 戻りコード（0 歩数 / 1 記録なし / 2 非対応 / 3 期限切れ / 4 取り消し / 5 失敗）の写像は
//! `decode_code` に任せ、値域の検査もそこで行う。
//!
//! 守る範囲: 1 プロセス内の多重起動の拒否（許可シートが残る間を含む）、期限の共有（単調時計）、
//! 期限切れでの `stopQuery`、失敗・拒否を 0 歩へ畳まないこと。守らない範囲: 期限後に許可シートで
//! 選ばれた結果は次の要求で反映される。
//! 再描画: OS の completion / handler は semaphore で同じワーカーへ結果を戻すだけで、egui の共有状態を
//! 書かない。結果はワーカーの戻り値として `CompletionSender::send`（直後に `ctx.request_repaint()`）を
//! 通るため、`crate::ios::request_repaint` の明示呼び出しは要らない（pedometer と同じ根拠）。
//!
//! 未確認（#1059 で実機確認）: 許可シートの挙動、拒否が「記録が見えない」になること、0 歩、端末ロック中、
//! シミュレータ（entitlement 無し）で成功にならないこと。

use std::sync::Arc;

#[cfg(target_os = "ios")]
use std::time::Duration;

use fandhe_device::health_data::HealthData;
#[cfg(not(target_os = "ios"))]
use fandhe_device::health_data::UnsupportedHealthData;
#[cfg(target_os = "ios")]
use fandhe_device::health_data::{decode_code, HealthDataFailure, HealthSteps};

// SAFETY: `ios/fandhe_ios_health_data.m` が定義する C ABI（`build.rs` が iOS ターゲットのみ・
// feature `device` 有効時のみコンパイル・リンクする）。契約は同 .m 冒頭コメント。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_health_data_read(timeout_ms: u64, out_steps: *mut f64) -> i32;
}

/// HealthKit による `HealthData` 実装。
#[cfg(target_os = "ios")]
struct IosHealthData;

#[cfg(target_os = "ios")]
impl HealthData for IosHealthData {
    #[allow(unsafe_code)]
    fn read_today_steps(&self, timeout: Duration) -> Result<HealthSteps, HealthDataFailure> {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let mut steps: f64 = 0.0;
        // SAFETY: `steps` はこの呼び出しの間だけ有効なスタック変数。`.m` は戻る前に呼び出しスレッド上で
        // のみ書き込み、遅れて来る OS コールバックは自身の `__block` 領域にしか触れない。
        // 戻りコードが 0 以外のとき `steps` は読まれない。
        let code = unsafe { fandhe_ios_health_data_read(timeout_ms, &mut steps) };
        decode_code(code, steps)
    }
}

/// ヘルスデータの実装を返す。ホストの test ビルドでは非対応を返す。
pub(crate) fn implementation() -> Arc<dyn HealthData> {
    #[cfg(target_os = "ios")]
    {
        Arc::new(IosHealthData)
    }
    #[cfg(not(target_os = "ios"))]
    {
        Arc::new(UnsupportedHealthData)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "ios"))]
    #[test]
    fn host_build_is_unsupported() {
        use fandhe_device::health_data::HealthDataFailure;
        use std::time::Duration;
        assert_eq!(
            super::implementation().read_today_steps(Duration::from_millis(1)),
            Err(HealthDataFailure::Unsupported)
        );
    }
}
