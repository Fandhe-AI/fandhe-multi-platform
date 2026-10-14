//! iOS 向けバッジ設定実装（Issue #1361）。`UNUserNotificationCenter.setBadgeCount`
//! （`ios/fandhe_ios_app_badge.m`、`build.rs` が iOS 分岐の feature `device` 有効時にコンパイル）
//! を `unsafe extern "C"` 1 関数で呼ぶ。`device/app_badge.rs` の iOS 経路（`begin_set`）から
//! 呼ばれ、dispatch・共通コア画面は #1360 で確定済みで触らない。契約は
//! `fandhe_device::app_badge` を参照。
//!
//! 結果は `.m` の completion から [`fandhe_rust_app_badge_outcome`] が `app_badge::bridge` へ
//! 置く（pull 型）。結果コード表の SSOT は [`outcome_from_code`]。
//!
//! - 0 は消去で許可要求をしない。1 以上は `.badge` のみ要求し、`badgeSetting` を確認してから設定
//! - 期限は設けない（許可ダイアログの応答待ちに上限を置かない。通知の iOS 実装と同じ）
//! - 守らない範囲: 通知機能（#1005）で `.alert` だけ先に許可した端末ではバッジ要求の
//!   ダイアログが再表示されず「許可されていません」になることがある（設定アプリで変更可）。
//!   設定後に OS や利用者がバッジを消した場合の表示の食い違いも守らない
//!
//! `NSError` の内容はログ・画面へ出さない（security.md A09）。

use fandhe_device::app_badge::BadgeOutcome;
#[cfg(target_os = "ios")]
use fandhe_device::app_badge::BadgeValue;
use fandhe_device::DeviceError;

// `ios/fandhe_ios_app_badge.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_app_badge_set(request_id: u64, value: u32) -> i32;
}

/// 設定を依頼する。`Ok` は受理済みで結果は後から `bridge::deliver` で届く。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub(crate) fn begin_set(request_id: u64, value: BadgeValue) -> Result<(), DeviceError> {
    // SAFETY: 引数は値渡しの整数のみでポインタを経由しない。`.m` 側は main queue へ渡して
    // 即 return し、結果は `fandhe_rust_app_badge_outcome` でちょうど 1 回届く。
    let code = unsafe { fandhe_ios_app_badge_set(request_id, value.get()) };
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("app_badge set"))
    }
}

/// `.m` の completion（main queue 上）から呼ばれる。結果を置いた直後に次フレームを起こす
/// （入力が無いと egui が描画せず画面が処理中のまま止まるため、契約表 4）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_app_badge_outcome(request_id: u64, value: u32, code: i32) {
    super::app_badge::bridge::deliver(request_id, value, outcome_from_code(code));
    crate::ios::request_repaint();
}

/// `.m` の結果コードを契約の結果へ写す（SSOT）。成功になるのは 0 と 1（キャンセル）のみ。
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
pub(crate) fn outcome_from_code(code: i32) -> Result<BadgeOutcome, DeviceError> {
    match code {
        0 => Ok(BadgeOutcome::Submitted),
        1 => Ok(BadgeOutcome::Cancelled),
        2 => Err(DeviceError::PermissionDenied("app_badge")),
        5 => Err(DeviceError::Unsupported("app_badge")),
        _ => Err(DeviceError::Internal("app_badge set")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_mapping() {
        assert!(matches!(outcome_from_code(0), Ok(BadgeOutcome::Submitted)));
        assert!(matches!(outcome_from_code(1), Ok(BadgeOutcome::Cancelled)));
        assert!(matches!(
            outcome_from_code(2),
            Err(DeviceError::PermissionDenied("app_badge"))
        ));
        assert!(matches!(
            outcome_from_code(5),
            Err(DeviceError::Unsupported("app_badge"))
        ));
        for c in [3, 4, 6, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_code(c),
                Err(DeviceError::Internal("app_badge set"))
            ));
        }
    }
}
