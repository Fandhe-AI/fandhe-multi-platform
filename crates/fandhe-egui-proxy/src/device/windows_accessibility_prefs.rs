//! Windows 向けアクセシビリティ設定実装（Issue #1308、親 #1132）。
//! `device/accessibility_prefs.rs::implementation` の Windows 経路から呼ばれ、WinRT
//! `UISettings.AnimationsEnabled` と `AccessibilitySettings.HighContrast` を UI スレッドで
//! 同期に読む（1 秒に 1 回。変化通知は購読しない）。
//!
//! 写像は `windows` クレートの型に依存しない純関数 [`prefs_from`] に切り出し、ホストの
//! `cargo test` で検査する（WinRT 呼び出しだけ内側の `cfg(target_os = "windows")` に閉じる。
//! `windows_network_status.rs` と同じ方針）。`unsafe` と `RoInitialize` は使わない
//! （`windows_text_scale.rs` と同じ前提）。HRESULT はログにも画面にも出さず固定ラベルの
//! `Internal` へ畳む（security.md A09）。
//!
//! - 視差軽減 = `AnimationsEnabled` の否定。コントラスト増加 = `HighContrast`（ハイコントラスト
//!   テーマの有効。Windows に「コントラスト増加」の単独設定は無いため近似）
//!
//! 既知の天井: unpackaged exe での動作は実機未確認（#1302）。ローカルに MSVC が無く型検査は
//! `x86_64-pc-windows-gnu` のみ。

use fandhe_device::accessibility_prefs::AccessibilityPrefs;
#[cfg(target_os = "windows")]
use fandhe_device::accessibility_prefs::AccessibilityPrefsSource;
#[cfg(target_os = "windows")]
use fandhe_device::DeviceError;

/// OS の 2 つの bool を契約の値へ写す純関数。
fn prefs_from(animations_enabled: bool, high_contrast: bool) -> AccessibilityPrefs {
    AccessibilityPrefs {
        reduce_motion: Some(!animations_enabled),
        increase_contrast: Some(high_contrast),
    }
}

/// WinRT による実装。状態を持たないため呼び出しごとに設定オブジェクトを生成する。
#[cfg(target_os = "windows")]
pub(crate) struct WindowsAccessibilityPrefs;

#[cfg(target_os = "windows")]
impl AccessibilityPrefsSource for WindowsAccessibilityPrefs {
    fn read(&self) -> Result<AccessibilityPrefs, DeviceError> {
        use windows::UI::ViewManagement::{AccessibilitySettings, UISettings};
        const FAILED: DeviceError = DeviceError::Internal("accessibility_prefs read");
        let animations = UISettings::new()
            .and_then(|s| s.AnimationsEnabled())
            .map_err(|_| FAILED)?;
        let high_contrast = AccessibilitySettings::new()
            .and_then(|s| s.HighContrast())
            .map_err(|_| FAILED)?;
        Ok(prefs_from(animations, high_contrast))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animations_off_means_reduce_motion_on() {
        let p = prefs_from(false, false);
        assert_eq!(p.reduce_motion, Some(true));
        assert_eq!(p.increase_contrast, Some(false));
    }

    #[test]
    fn animations_on_and_high_contrast() {
        let p = prefs_from(true, true);
        assert_eq!(p.reduce_motion, Some(false));
        assert_eq!(p.increase_contrast, Some(true));
    }
}
