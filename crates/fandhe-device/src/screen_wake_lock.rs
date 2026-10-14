//! 画面スリープの抑止（Phase 11 の拡充ループ、親 #1129、Issue #1278、対応表
//! `docs/device/capabilities/screen-wake-lock.json`）。
//!
//! 扱う範囲は「アプリが前面にいる間、画面の自動消灯・スリープを止める / 戻す」ことだけ。
//! 対象は iOS / Android / macOS / Windows / Web。取得・解除はどちらも即時で、結果を待つ
//! 要求を持たない一発呼び出し型。`fandhe-egui-proxy` の橋渡し層が UI スレッドから
//! [`ScreenWakeLock::set_keep_awake`] を呼び、[`ScreenWakeLock::is_held`] で実際に効いているかを
//! 読み戻して画面（`fandhe_core::screens::wake_lock_toggle`）へ返す。
//!
//! # 契約
//!
//! - **「要求した」と「効いている」を分ける**: 画面は [`ScreenWakeLock::is_held`] が真のときだけ
//!   「抑止しています」と出す（`device-bridge-contracts.md`「1. ライフサイクル境界」
//!   「6. 嘘の成功」）
//! - **冪等**: 取得済みでの再取得は新しい資源を作らず `Ok`。未取得での解除は `Ok`。
//!   二重取得でアサーション等を漏らさない（同「9. 失敗時のリソース解放」）
//! - **失敗時の `is_held`**: 取得に失敗したら `false` のまま。解除に失敗したら `true` のまま
//!   （効いているものを効いていないと見せない）
//! - **エラー種別**: 非対応 → `Err(Unsupported)`、OS 呼び出しの失敗 → `Err(Internal)`。
//!   権限は不要で、利用者がキャンセルする経路も無いので `PermissionDenied` / `Cancelled` は
//!   返さない。三値（同「10. キャンセル・正常終了・失敗」）のうちキャンセルは該当なしで、
//!   成功 / 失敗の二値になる
//! - **`is_held` の意味は OS ごとに違う**: macOS はアサーション ID の保持、Windows は自前の
//!   記録（`SetThreadExecutionState` に読み戻し API が無い）、iOS は `isIdleTimerDisabled` の
//!   読み戻し、Android は UI スレッドで読んだ Window フラグ、Web は `WakeLockSentinel.released`
//! - **スレッド**: UI スレッドから呼ぶ。Windows の `SetThreadExecutionState` は呼んだ
//!   スレッドに紐づくため、ワーカースレッドで取得 / 解除しない
//!
//! # 守る範囲と守らない範囲
//!
//! 守るのは、1 プロセス内で解除が漏れないこと、「効いている」と表示している間は実際に抑止して
//! いること（Web は `release` イベントで即反映）、失敗・非対応を成功と見せないこと。
//! 守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）は、読み戻し API が無い OS で
//! 他アプリ / OS が外から抑止を外したときの追従、利用者による明示スリープ・蓋閉じ、
//! プロセス終了時の後片付け（macOS はアサーションがプロセス終了で解放され、Windows は
//! スレッド終了でフラグが消える。OS が代わりに片付けるため同期させない）。いずれも表示が
//! 一時的に食い違うだけで、データ消失・資源漏れ・嘘の成功には至らない。

use crate::DeviceError;

/// 画面スリープ抑止の契約（モジュール doc の「契約」節）。
pub trait ScreenWakeLock: Send + Sync {
    /// `true` で抑止を取り、`false` で外す。冪等。
    fn set_keep_awake(&self, keep: bool) -> Result<(), DeviceError>;

    /// 抑止が実際に効いているか（モジュール doc の「`is_held` の意味」）。
    fn is_held(&self) -> bool;
}

/// 対象外 OS（Linux 等）・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedScreenWakeLock;

impl ScreenWakeLock for UnsupportedScreenWakeLock {
    fn set_keep_awake(&self, _keep: bool) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("screen-wake-lock"))
    }

    fn is_held(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_is_not_a_fake_success() {
        for keep in [true, false] {
            let err = UnsupportedScreenWakeLock.set_keep_awake(keep).unwrap_err();
            assert!(matches!(err, DeviceError::Unsupported(_)));
        }
        assert!(!UnsupportedScreenWakeLock.is_held());
    }
}
