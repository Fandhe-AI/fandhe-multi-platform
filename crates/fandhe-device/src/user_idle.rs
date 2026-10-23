//! Phase 11 の拡充ループ（親 #125、Issue #631）: 無操作時間（最後の入力からの経過）を 1 回読む契約。
//! `fandhe-egui-proxy` の `device/user_idle.rs` が 1 秒間隔の pull で [`UserIdle::idle_for`] を
//! 呼び、各 OS 実装（macOS: `CGEventSourceSecondsSinceLastEventType`、Windows: `GetLastInputInfo`
//! + `GetTickCount`）が本トレイトを実装する。iOS・Android は OS 全体の値を返す公開 API が無く
//! 対応表で `no`。Web は `fandhe-web-dom` 側（#632）で Idle Detection API を使い、本トレイトは
//! 経由しない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: `Ok(経過時間)`
//! - 非対応: `Err(DeviceError::Unsupported("user_idle"))`
//! - 失敗: `Err(DeviceError::Internal("user_idle read"))`（OS の戻り値が不正・呼び出しが失敗）。
//!   NaN・負・未来の tick を 0 秒や前回値で埋めず、この失敗として返す（「操作中」という嘘を出さない）
//!
//! 権限ダイアログとキャンセルの経路は無い（同期 pull のみ）。macOS で権限ダイアログ（TCC）が出る
//! と分かった場合は、`PermissionDenied` を足す小さな PR を別に出す（親 #125 の決定）。
//!
//! # 時計（契約表 3）
//!
//! OS が管理する単調な値だけを使う。macOS は関数の戻り値、Windows は `GetTickCount` 同士の差。
//! 壁時計（`SystemTime`・FILETIME）は使わない。経過時間と状態はログへ出さない（在席を推測できる
//! ため、画面表示のみでプロセスの外へ送らない）。
//!
//! # 呼び出し契約
//!
//! UI スレッドから同期で呼ばれる。数 µs で戻り、ブロックしない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 嘘の値を出さない、非対応・許可なし・失敗を「操作中」と見せない、Web で経過秒を偽らない、
//! 1 プロセス内の順序、Web のリスナーと `AbortController` の解放、古い世代の結果を捨てる。
//!
//! 守らない: OS 間の「入力」の定義の差（合成入力・タブレット・ゲームパッド）、最大 1 秒の反映遅れ、
//! スリープ中の扱いの OS 差、別セッション（ファストユーザースイッチ・RDP）の入力、デスクトップの
//! 画面ロック、最小化中・App Nap 中の更新遅れ、Windows の 24.8 日超の無操作（現在より新しい
//! tick と区別できないため失敗表示になる）、Web の複数タブ・Chromium 系以外・許可ダイアログの
//! 応答待ちの期限。
//!
//! 並行性: 読み取りは UI スレッドの effect 1 本だけで、ワーカー・コールバック・購読を持たない。
//! 集約すべき並行経路は無い。

use std::time::Duration;

use crate::DeviceError;

/// 読み取り間隔（effect ドライバが使う）。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// 無操作時間の読み取り契約（module doc 参照）。
pub trait UserIdle: Send + Sync {
    /// 最後の入力からの経過時間を同期で読む。
    fn idle_for(&self) -> Result<Duration, DeviceError>;
}

/// 常に非対応を返す実装。API が無い OS が成功値（ダミー）へ倒れないための既定。
pub struct UnsupportedUserIdle;

impl UserIdle for UnsupportedUserIdle {
    fn idle_for(&self) -> Result<Duration, DeviceError> {
        Err(DeviceError::Unsupported("user_idle"))
    }
}

/// macOS が返す秒（`f64`）を経過時間へ。NaN・負・無限大・`Duration` に収まらない値は失敗
/// （0 や前回値で埋めない）。変換の前に検証する（契約表 5）。
pub fn idle_from_seconds(secs: f64) -> Result<Duration, DeviceError> {
    Duration::try_from_secs_f64(secs).map_err(|_| DeviceError::Internal("user_idle read"))
}

/// Windows の 32 bit tick（ms）2 つから経過時間を作る。`wrapping_sub` で約 49.7 日の折り返しに
/// 耐える。差が `i32::MAX` ms（約 24.8 日）を超えたら「現在より新しい tick」とみなして失敗
/// （0 秒にしない）。本物の 24.8 日超の無操作とは区別できない（守らない範囲）。
pub fn idle_from_ticks(now: u32, last_input: u32) -> Result<Duration, DeviceError> {
    let diff = now.wrapping_sub(last_input);
    if diff > i32::MAX as u32 {
        return Err(DeviceError::Internal("user_idle read"));
    }
    Ok(Duration::from_millis(u64::from(diff)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedUserIdle.idle_for(),
            Err(DeviceError::Unsupported("user_idle"))
        ));
    }

    #[test]
    fn seconds_valid_values_succeed() {
        assert_eq!(idle_from_seconds(0.0).unwrap(), Duration::ZERO);
        assert_eq!(idle_from_seconds(1.5).unwrap(), Duration::from_millis(1500));
    }

    #[test]
    fn seconds_invalid_values_fail() {
        for v in [f64::NAN, -1.0, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
            assert!(
                matches!(idle_from_seconds(v), Err(DeviceError::Internal(_))),
                "{v}"
            );
        }
    }

    #[test]
    fn ticks_normal_and_equal() {
        assert_eq!(
            idle_from_ticks(5000, 2000).unwrap(),
            Duration::from_millis(3000)
        );
        assert_eq!(idle_from_ticks(7, 7).unwrap(), Duration::ZERO);
    }

    #[test]
    fn ticks_wraparound_succeeds() {
        assert_eq!(
            idle_from_ticks(5, u32::MAX - 4).unwrap(),
            Duration::from_millis(10)
        );
    }

    #[test]
    fn ticks_future_value_fails_not_zero() {
        assert!(matches!(
            idle_from_ticks(100, 101),
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(
            idle_from_ticks(0, 1),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn ticks_boundary() {
        let max = i32::MAX as u32;
        assert_eq!(
            idle_from_ticks(max, 0).unwrap(),
            Duration::from_millis(u64::from(max))
        );
        assert!(idle_from_ticks(max + 1, 0).is_err());
    }
}
