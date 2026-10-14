//! iOS 向け画面スリープ抑止実装（Issue #1279）。`UIApplication.idleTimerDisabled`
//! （`ios/fandhe_ios_screen_wake_lock.m`、`build.rs` が iOS 分岐の feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 2 関数で書く / 読む。`unsafe` はこの呼び出しに閉じ
//! `// SAFETY:` で契約を明記する。`device/screen_wake_lock.rs` の iOS 経路が `implementation()`
//! を呼ぶ（dispatch・共通コア画面は #1278 で確定済みで触らない）。
//!
//! # 状態
//!
//! Rust 側は状態を持たない。`is_held` は OS の `isIdleTimerDisabled` の読み戻し。bool プロパティ
//! で資源を作らないため、二重設定・未設定での解除も冪等で漏れない（契約表 9）。読み書きは
//! main スレッドに限られ、そこで直列化されるため `Mutex` は置かない（契約表 2）。main 以外からの
//! `set_keep_awake` は `Internal`、`is_held` は false（effect は常に main から呼ぶので契約外）。
//!
//! # 再描画（契約表 4）
//!
//! OS コールバックが共有状態へ結果を入れる経路は無く、完了は `drive` が同期で読み戻して
//! `CompletionSender::send`（内部で `request_repaint`）が起こす。復帰時の observer は OS の値を
//! 要求へ揃えるだけで共有状態を変えないため、`crate::ios::request_repaint` は呼ばない（該当なし）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の解除漏れなし、失敗を成功と見せない。守らない: 背景にいる間は効かない
//! （利用者に画面が見えず実害なし）、他要因によるタイマー変更との一時的な食い違い。
//! 終了時はプロセス終了で OS が戻すため `device::shutdown` への追加は不要。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::screen_wake_lock::ScreenWakeLock;
use fandhe_device::DeviceError;

// `ios/fandhe_ios_screen_wake_lock.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_screen_wake_lock_set(keep: i32) -> i32;
    fn fandhe_ios_screen_wake_lock_is_held() -> i32;
}

/// `set` の戻りコード（0 = 成功、それ以外 = 失敗）を契約のエラー種別へ写す。
fn decode(code: i32) -> Result<(), DeviceError> {
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("screen-wake-lock"))
    }
}

/// `is_held` の戻りコード。1 のときだけ「効いている」（main 以外の 2・未知値は false）。
fn decode_held(code: i32) -> bool {
    code == 1
}

/// iOS の `ScreenWakeLock` 実装（`idleTimerDisabled`）。
#[cfg(target_os = "ios")]
struct IosScreenWakeLock;

#[cfg(target_os = "ios")]
impl ScreenWakeLock for IosScreenWakeLock {
    #[allow(unsafe_code)]
    fn set_keep_awake(&self, keep: bool) -> Result<(), DeviceError> {
        // SAFETY: 引数は値渡しの i32 のみでポインタを経由しない。`.m` 側で main スレッドを
        // 検査し、main 以外では UIKit に触れず 2 を返す。
        let code = unsafe { fandhe_ios_screen_wake_lock_set(i32::from(keep)) };
        decode(code)
    }

    #[allow(unsafe_code)]
    fn is_held(&self) -> bool {
        // SAFETY: 引数なし・戻りは値渡しの i32。main 検査は `.m` 側で行う。
        decode_held(unsafe { fandhe_ios_screen_wake_lock_is_held() })
    }
}

/// プロセスで 1 個の iOS 実装を返す。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn ScreenWakeLock> {
    Arc::new(IosScreenWakeLock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_maps_nonzero_to_internal() {
        assert!(decode(0).is_ok());
        for code in [2, -1] {
            assert!(matches!(
                decode(code),
                Err(DeviceError::Internal("screen-wake-lock"))
            ));
        }
    }

    #[test]
    fn decode_held_only_true_for_one() {
        assert!(decode_held(1));
        assert!(!decode_held(0));
        assert!(!decode_held(2));
        assert!(!decode_held(99));
    }
}
