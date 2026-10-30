//! ヘルスデータ（今日の歩数を 1 回読む）の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/health-data.json`、親 #149、Issue #1089）。
//!
//! 扱う範囲は「今日 0 時から現在までの歩数の合計をヘルスストアから 1 回読む」ことだけ。
//! 機能名は「読み書き」だが、書き込み・他のデータ型・期間指定・背景配信は扱わない。
//! 対象は iOS（HealthKit）と Android（API 34 以上のフレームワーク API `HealthConnectManager`。
//! 実体は #1100）。macOS・Windows・Web は対象外。
//! `fandhe-egui-proxy` の `device/health_data.rs` がワーカースレッドで [`HealthData::read_today_steps`]
//! を呼ぶ（ブロッキングの契約）。
//!
//! # 契約
//!
//! - 成功は 2 形。[`HealthSteps::Steps`]（0 歩を含む）と [`HealthSteps::NoVisibleData`]。
//!   `NoVisibleData` は「記録が無い」と「読み取りを拒否された」を区別しない（拒否は OS がアプリに
//!   隠すため）。0 歩にも失敗にも畳まない。OS のデータ無しの応答と「合計が nil」はここへ写す。
//!   この向きは公式文書に基づいて固定したもので、レビューの指摘で反転させない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）
//! - 失敗は [`HealthDataFailure`]。`DeviceError` に variant を足すと既存の網羅 match が壊れるため
//!   機能固有の enum にする（`pedometer::PedometerFailure` と同じ判断）。判別できない拒否を
//!   `PermissionDenied` と断定する種別は作らない
//! - 期限は [`READ_TIMEOUT`] が唯一の値。初回の許可シートへの応答時間を含み、許可待ちと集計待ちで
//!   同じ期限を共有する（段ごとに持たない）。起点は呼び出しの入口で単調時計で測る。
//!   実装は期限切れの前に走っている集計を取り消してから戻り、期限後に届いた結果を成功にしない
//! - **三値**（類型 10）: 成功は `Steps`・`NoVisibleData`、取り消しは [`HealthDataFailure::Canceled`]
//!   （利用者が許可シートを閉じた）、失敗は `Unsupported`・`Timeout`（許可シートの放置を含む）・
//!   `Failed`（端末ロック中・entitlement 不足・多重起動・引数不正・未知のコードを含む）
//! - 歩数をログ・エラー・`Debug` に出さない（健康情報）
//! - 許可の状態をアプリ内に持たず、要求のたびに OS に求め直す
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内で要求を 1 件ずつ処理すること（画面のボタン外し + iOS 側の多重起動の拒否）、
//! 0 歩・記録が見えない・失敗の区別、嘘の成功を出さないこと（error・期限切れを 0 歩にしない）、
//! 期限切れで戻る前に集計を止めること。許可シート表示中にアプリが Inactive になっても要求を
//! 中断扱いにしない（期限だけで決める）。
//! 守らない: 拒否と記録なしの区別、pedometer の値との一致（HealthKit は集約値）、読み取った直後に
//! 追加された記録、日付をまたぐ瞬間の要求、期限後に許可シートで選ばれた結果（次の要求で反映）、
//! 複数プロセス。Android 固有の守らない範囲は「Android の写像」節に書く。
//!
//! 使わない entitlement（`healthkit.access`・`background-delivery`）は足さない方針。
//!
//! # Android の写像（#1100 が従う契約）
//!
//! - API 33 以下、または `HealthConnectManager` を取得できない場合は `Unsupported`（コード 2）。
//!   Jetpack の `connect-client` は使わない
//! - 許可が得られない場合（拒否・許可画面を閉じた・拒否済みで画面が出ない）と読み取り時の
//!   `SecurityException` は区別せず `NoVisibleData`（コード 1）。「許可なし」の種別は作らず、
//!   0 歩にも失敗にもしない。iOS と同じ向きで、レビューの指摘で反転させない。したがって
//!   Android は `Canceled` を返さない（三値の取り消しは iOS 専用）
//! - `aggregate`（`StepsRecord.STEPS_COUNT_TOTAL`）が null（記録なし）なら `NoVisibleData`、
//!   値があれば `Steps`（値域は [`decode_code`] / [`steps_from`] で検査）
//! - それ以外の例外（`HealthConnectException` 等）と JNI の失敗は `Failed`
//! - 期限: [`READ_TIMEOUT`] を許可画面の待ちと集計の待ちで共有し、起点は Rust 側の入口（単調時計）。
//!   `aggregate` に取り消し API は無いため、期限後に届いた結果は要求 id の照合（`RequestTracker`）で
//!   捨て、成功にしない。資源は保持しない
//! - ライフサイクル: 許可画面の表示に伴う `onPause` / `onStop` では中断扱いにせず期限だけで決める。
//!   Activity 破棄時の保留中の要求は `Failed` で確定する。許可画面の結果は次の要求で反映される。
//!   保留中の再要求は `Failed`
//! - 歩数・例外メッセージを logcat・`Debug` に出さない
//! - コード表の正は [`decode_code`]。Java 側は同じ番号を定数で持ち、その doc で本関数を参照する
//! - 守らない範囲（Android 固有）: 期限後に届いた `aggregate` の結果、既定で許可時点の 30 日前
//!   までしか読めないこと（今日の分には影響しない）
//! - `READ_HEALTH_DATA_IN_BACKGROUND`・`READ_HEALTH_DATA_HISTORY` の権限は足さない方針

use std::fmt;
use std::time::Duration;

/// 歩数読み取りの期限（Rust 側の唯一の値）。初回の許可シートへの応答時間を含む
/// （`music_library::LIST_TIMEOUT` と同じ考え方）。
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// 読み取りの成功値。`Debug` は歩数を出さない。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HealthSteps {
    /// 今日の歩数の合計（0 歩は正当な成功値）。
    Steps(u64),
    /// 記録が見えない（記録が無い、または読み取りが拒否されている。OS は区別させない）。
    NoVisibleData,
}

impl fmt::Debug for HealthSteps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Steps(_) => f.write_str("Steps(..)"),
            Self::NoVisibleData => f.write_str("NoVisibleData"),
        }
    }
}

/// 読み取りの失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthDataFailure {
    /// 端末・OS・対象 OS 外でヘルスストアを使えない。
    Unsupported,
    /// 利用者が許可シートを閉じた（失敗ではなく取り消し）。
    Canceled,
    /// 期限内に結果が届かない（許可シートの放置を含む）。
    Timeout,
    /// 上記以外の OS 呼び出し失敗（値域違反・未知のコードを含む）。
    Failed,
}

/// ヘルスデータ読み取りの契約（モジュール doc の「契約」節）。
pub trait HealthData: Send + Sync {
    /// 今日 0 時から現在までの歩数の合計を 1 回読む。呼び出し側がワーカースレッドで呼ぶ
    /// ブロッキング呼び出しで、`timeout` 以内に必ず戻る。
    fn read_today_steps(&self, timeout: Duration) -> Result<HealthSteps, HealthDataFailure>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedHealthData;

impl HealthData for UnsupportedHealthData {
    fn read_today_steps(&self, _timeout: Duration) -> Result<HealthSteps, HealthDataFailure> {
        Err(HealthDataFailure::Unsupported)
    }
}

/// OS が返した歩数（f64）の値域検査。検査は丸めの前に行い、NaN・∞・負・2^64 以上は `Failed`。
/// `as` は飽和するため上限の明示検査が必須。
pub fn steps_from(raw: f64) -> Result<u64, HealthDataFailure> {
    if !raw.is_finite() || raw < 0.0 {
        return Err(HealthDataFailure::Failed);
    }
    let rounded = raw.round();
    if rounded >= 18_446_744_073_709_551_616.0 {
        return Err(HealthDataFailure::Failed);
    }
    Ok(rounded as u64)
}

/// FFI の戻りコードを結果へ写す唯一の箇所（`ios/fandhe_ios_health_data.m` と Android の Java 側が同じ番号で返す）。
/// `steps` は `code == 0` のときだけ読む。0 は歩数、1 は記録なし、2 は非対応、3 は期限切れ、
/// 4 は取り消し、それ以外（5・負値・未知）は `Failed`。
pub fn decode_code(code: i32, steps: f64) -> Result<HealthSteps, HealthDataFailure> {
    match code {
        0 => steps_from(steps).map(HealthSteps::Steps),
        1 => Ok(HealthSteps::NoVisibleData),
        2 => Err(HealthDataFailure::Unsupported),
        3 => Err(HealthDataFailure::Timeout),
        4 => Err(HealthDataFailure::Canceled),
        _ => Err(HealthDataFailure::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedHealthData.read_today_steps(READ_TIMEOUT),
            Err(HealthDataFailure::Unsupported)
        );
    }

    #[test]
    fn decode_code_maps_every_code() {
        assert_eq!(decode_code(0, 12.0), Ok(HealthSteps::Steps(12)));
        assert_eq!(decode_code(0, 0.0), Ok(HealthSteps::Steps(0)));
        assert_eq!(decode_code(1, 0.0), Ok(HealthSteps::NoVisibleData));
        assert_eq!(decode_code(2, 0.0), Err(HealthDataFailure::Unsupported));
        assert_eq!(decode_code(3, 5.0), Err(HealthDataFailure::Timeout));
        assert_eq!(decode_code(4, 5.0), Err(HealthDataFailure::Canceled));
        for c in [5, -1, 99] {
            assert_eq!(decode_code(c, 5.0), Err(HealthDataFailure::Failed));
        }
        assert_eq!(decode_code(0, f64::NAN), Err(HealthDataFailure::Failed));
    }

    #[test]
    fn steps_from_rejects_out_of_range_before_rounding() {
        for v in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            -0.4,
            18_446_744_073_709_551_616.0,
        ] {
            assert_eq!(steps_from(v), Err(HealthDataFailure::Failed), "{v}");
        }
    }

    #[test]
    fn steps_from_rounds_valid_values() {
        assert_eq!(steps_from(-0.0), Ok(0));
        assert_eq!(steps_from(0.4), Ok(0));
        assert_eq!(steps_from(0.5), Ok(1));
        assert!(steps_from(18_446_744_073_709_549_568.0).is_ok());
    }

    #[test]
    fn debug_does_not_leak_steps() {
        assert!(!format!("{:?}", HealthSteps::Steps(12345)).contains("12345"));
    }
}
