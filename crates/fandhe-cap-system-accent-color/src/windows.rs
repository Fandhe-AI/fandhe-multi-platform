//! Windows 向けシステムのアクセント色実装（Issue #421、親 #103）。
//! `driver.rs::platform_system_accent_color` の Windows 経路から呼ばれ、WinRT
//! `UISettings.GetColorValue(UIColorType.Accent)` を UI スレッドで同期に読む
//! （1 秒に 1 回。`ColorValuesChanged` は購読しない）。
//!
//! `unsafe` と `RoInitialize` は使わない（`windows_text_scale.rs` / `windows_accessibility_prefs.rs`
//! と同じ前提）。HRESULT と色の値はログにも画面にも出さず、固定ラベルの `Internal` へ畳む
//! （security.md A09）。アルファは捨てる。
//!
//! 既知の天井: unpackaged exe での動作は実機未確認（#415）。

use crate::contract::{AccentColorSource, Rgb};
use crate::error::SystemAccentColorError;
use windows::UI::ViewManagement::{UIColorType, UISettings};

/// [`AccentColorSource`] の Windows 実装。状態を持たず、呼び出しごとに `UISettings` を生成する。
pub(crate) struct WindowsAccentColor;

impl AccentColorSource for WindowsAccentColor {
    fn read(&self) -> Result<Rgb, SystemAccentColorError> {
        let c = UISettings::new()
            .and_then(|s| s.GetColorValue(UIColorType::Accent))
            .map_err(|_| SystemAccentColorError::Internal)?;
        Ok(Rgb {
            r: c.R,
            g: c.G,
            b: c.B,
        })
    }
}
