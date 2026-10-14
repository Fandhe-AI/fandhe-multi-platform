//! macOS / iOS 向けアクセシビリティ設定実装（Issue #1308 が macOS、#1309 が iOS、親 #1132）。
//! `device/accessibility_prefs.rs::implementation` の macOS 経路と、iOS 経路
//! （`ios_accessibility_prefs.rs` 経由）から呼ばれ、`apple/fandhe_apple_accessibility_prefs.m`
//! （`build.rs` が feature `device` 有効時にコンパイル）を C ABI で呼ぶ。
//! `unsafe` は引数なし・戻り値 int のみの FFI 1 箇所に閉じる。
//!
//! # packed 値の写像
//!
//! bit0-1 = 視差軽減、bit2-3 = コントラスト増加（各 0 = 不明 → `None`、1 = オフ、2 = オン）。
//! 負値・各 2 ビットの値 3・上位ビットの残りは `Internal`（成功にしない、契約表 6）。
//! 値をログへ出さない。
//!
//! 読み取りは `NSWorkspace` / `UIAccessibility` の同期読みで購読を持たず、保持する資源も無い
//! （契約表 1・2・3 は該当なし）。
//! iOS の UIKit は main スレッド前提で、ガードは `.m` 側（main 以外は負値 → `Internal`）。

// ホストの test ビルドでは純関数がテストからのみ使われる。
#![cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]

use fandhe_device::accessibility_prefs::AccessibilityPrefs;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use fandhe_device::accessibility_prefs::AccessibilityPrefsSource;
use fandhe_device::DeviceError;

const FAILED_MSG: &str = "accessibility_prefs read";

fn flag(two_bits: i32) -> Result<Option<bool>, DeviceError> {
    match two_bits {
        0 => Ok(None),
        1 => Ok(Some(false)),
        2 => Ok(Some(true)),
        _ => Err(DeviceError::Internal(FAILED_MSG)),
    }
}

/// `.m` が詰めた値を契約の値へ戻す純関数（ホストの `cargo test` で検査する）。
fn prefs_from_packed(packed: i32) -> Result<AccessibilityPrefs, DeviceError> {
    if packed < 0 || packed >> 4 != 0 {
        return Err(DeviceError::Internal(FAILED_MSG));
    }
    Ok(AccessibilityPrefs {
        reduce_motion: flag(packed & 0b11)?,
        increase_contrast: flag((packed >> 2) & 0b11)?,
    })
}

// SAFETY: `apple/fandhe_apple_accessibility_prefs.m` が定義する C ABI。引数なし・戻り値は int のみで
// 所有権の移転もポインタの受け渡しも無い。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_accessibility_prefs_read() -> i32;
}

/// `NSWorkspace`（macOS）/ `UIAccessibility`（iOS）による実装。状態は持たない。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) struct AppleAccessibilityPrefs;

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl AccessibilityPrefsSource for AppleAccessibilityPrefs {
    #[allow(unsafe_code)]
    fn read(&self) -> Result<AccessibilityPrefs, DeviceError> {
        // SAFETY: 上記 extern 宣言のとおり、副作用は OS 設定の読み取りのみ。
        let packed = unsafe { fandhe_apple_accessibility_prefs_read() };
        prefs_from_packed(packed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(rm: Option<bool>, ic: Option<bool>) -> AccessibilityPrefs {
        AccessibilityPrefs {
            reduce_motion: rm,
            increase_contrast: ic,
        }
    }

    #[test]
    fn decodes_each_state() {
        assert_eq!(prefs_from_packed(0).unwrap(), p(None, None));
        assert_eq!(
            prefs_from_packed(1 | (2 << 2)).unwrap(),
            p(Some(false), Some(true))
        );
        assert_eq!(
            prefs_from_packed(2 | (1 << 2)).unwrap(),
            p(Some(true), Some(false))
        );
    }

    #[test]
    fn invalid_values_are_errors_not_off() {
        for v in [-1, i32::MIN, 3, 3 << 2, 1 << 4, i32::MAX] {
            assert!(
                matches!(prefs_from_packed(v), Err(DeviceError::Internal(_))),
                "v={v}"
            );
        }
    }
}
