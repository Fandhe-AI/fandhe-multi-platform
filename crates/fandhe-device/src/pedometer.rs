//! 歩数（累積歩数を 1 回取得）の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/pedometer.json`、親 #1119）。
//!
//! 扱う範囲は「ボタンを押すと累積歩数を 1 回取得する」ことだけ。履歴・背景取得・継続 push、
//! HealthKit / Health Connect（health-data）は扱わない。対象は Windows（#1247）・
//! iOS（#1248）・Android（#1249）。macOS・Web は対応表で `no`。
//! `fandhe-egui-proxy` の `device/pedometer.rs` がワーカースレッドで [`Pedometer::read_steps`]
//! を呼ぶ（ブロッキングの契約）。
//!
//! # 契約
//!
//! - 結果は成功（歩数）か [`PedometerFailure`]。`DeviceError` に variant を足すと既存の網羅
//!   match が壊れるため機能固有の enum にする（`location::LocationFailure` と同じ判断）
//! - **0 歩は正当な成功値**。非対応・権限拒否・値が届かない・失敗を 0 歩へ畳まない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）
//! - 値の起点は OS ごとに違い揃えない。Windows はセンサーの種類別累積の合計、iOS は今日 0 時から、
//!   Android は再起動後の累積
//! - 期限は [`READ_TIMEOUT`] が唯一の値。実装は引数で受けた `timeout` 以内に必ず戻り、
//!   戻る前に登録したリスナー・非同期操作を解放・取り消す（類型 9）
//! - **三値**: 利用者キャンセルの経路は無い。権限ダイアログの拒否は
//!   [`PedometerFailure::PermissionDenied`]、期限切れは [`PedometerFailure::NoReading`]
//!   （失敗の一種として表示し、成功にもキャンセルにもしない）
//! - 歩数・HRESULT 等の OS 固有値をログへ出さない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内で要求を 1 件ずつ処理すること、0 歩と未取得の区別、嘘の成功を出さないこと、
//! 期限後に資源を残さないこと。守らない: OS 間の値の一致、センサー遅延による値の鮮度、
//! 画面離脱後に届いた値（呼び出し側が世代照合で捨てる）。

use std::time::Duration;

/// 歩数取得の期限（Rust 側の唯一の値）。Android `TYPE_STEP_COUNTER` の最大 10 秒遅延に
/// 余裕を足した値で、各 OS 実装は別の期限を持たない。
pub const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// 歩数取得の失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PedometerFailure {
    /// 端末にセンサーが無い・API が無い・OS 版が足りない。
    Unsupported,
    /// 利用者または OS 設定が歩数の利用を拒否した。
    PermissionDenied,
    /// 期限内に値が届かない、または OS が値を持っていない。0 歩にはしない。
    NoReading,
    /// 上記以外の OS 呼び出し失敗（値域違反を含む）。
    Failed,
}

/// 歩数取得の契約（モジュール doc の「契約」節）。
pub trait Pedometer: Send + Sync {
    /// 累積歩数を 1 回取得する。呼び出し側がワーカースレッドで呼ぶブロッキング呼び出しで、
    /// `timeout` 以内に必ず戻る。
    fn read_steps(&self, timeout: Duration) -> Result<u64, PedometerFailure>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedPedometer;

impl Pedometer for UnsupportedPedometer {
    fn read_steps(&self, _timeout: Duration) -> Result<u64, PedometerFailure> {
        Err(PedometerFailure::Unsupported)
    }
}

/// OS が返した種類別（または単一）の累積歩数を合算する値域検査。
/// 空は `NoReading`、負値・overflow は `Failed`、全要素 0 は `Ok(0)`（正当な 0 歩）。
pub fn total_steps(per_kind: impl IntoIterator<Item = i64>) -> Result<u64, PedometerFailure> {
    let mut any = false;
    let mut total: u64 = 0;
    for v in per_kind {
        any = true;
        let v = u64::try_from(v).map_err(|_| PedometerFailure::Failed)?;
        total = total.checked_add(v).ok_or(PedometerFailure::Failed)?;
    }
    if any {
        Ok(total)
    } else {
        Err(PedometerFailure::NoReading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedPedometer.read_steps(READ_TIMEOUT),
            Err(PedometerFailure::Unsupported)
        );
    }

    #[test]
    fn total_steps_empty_is_no_reading() {
        assert_eq!(total_steps([]), Err(PedometerFailure::NoReading));
    }

    #[test]
    fn total_steps_zero_is_valid_success() {
        assert_eq!(total_steps([0, 0]), Ok(0));
    }

    #[test]
    fn total_steps_sums() {
        assert_eq!(total_steps([10, 5, 1]), Ok(16));
    }

    #[test]
    fn total_steps_rejects_negative_and_overflow() {
        assert_eq!(total_steps([3, -1]), Err(PedometerFailure::Failed));
        assert_eq!(
            total_steps([i64::MAX, i64::MAX, i64::MAX]),
            Err(PedometerFailure::Failed)
        );
    }
}
