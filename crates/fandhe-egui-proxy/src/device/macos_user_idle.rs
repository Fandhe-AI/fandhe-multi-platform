//! macOS 向け無操作時間実装（Issue #631）。`macos/fandhe_macos_user_idle.m`
//! （`CGEventSourceSecondsSinceLastEventType`、`build.rs` が macOS ターゲットの feature `device`
//! 有効時にコンパイル・リンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに
//! 閉じ `// SAFETY:` で契約を明記する。`device/user_idle.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::user_idle`。
//!
//! 戻り値の検証（NaN・負・無限大）は `idle_from_seconds` が行い、不正値は 0 や前回値で埋めずに
//! 失敗として返す。
//!
//! # 既知の天井
//!
//! - 権限ダイアログ（TCC）の有無は未確認。出る場合は `PermissionDenied` を足す別 PR で対応する
//! - 実機の目視確認は #627

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_user_idle.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイル・リンクする）が定義する C ABI。引数なしで、契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_user_idle_seconds() -> f64;
    }

    #[allow(unsafe_code)]
    pub(super) fn seconds() -> f64 {
        // SAFETY: 引数もポインタも取らず、呼び出し間で状態を保持しない純粋な読み取り。
        unsafe { fandhe_macos_user_idle_seconds() }
    }
}

#[cfg(target_os = "macos")]
struct MacUserIdle;

#[cfg(target_os = "macos")]
impl fandhe_device::user_idle::UserIdle for MacUserIdle {
    fn idle_for(&self) -> Result<std::time::Duration, fandhe_device::DeviceError> {
        fandhe_device::user_idle::idle_from_seconds(ffi::seconds())
    }
}

/// macOS の `UserIdle` 実装を返す（`device/user_idle.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::user_idle::UserIdle> {
    Box::new(MacUserIdle)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// 実 OS を 1 回読めること（リンクと FFI の疎通。値は環境依存のため成功だけ見る）。
    #[test]
    fn real_read_succeeds() {
        assert!(implementation().idle_for().is_ok());
    }
}
