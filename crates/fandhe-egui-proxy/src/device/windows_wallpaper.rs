//! Windows 向け壁紙設定実装（Issue #970、親 #188）。`device/wallpaper.rs` の Windows 経路から
//! UI スレッドで呼ばれ、`SystemParametersInfoW(SPI_SETDESKWALLPAPER)` を 1 回呼ぶ。契約は
//! `fandhe_device::wallpaper`。素の exe で動き、権限・package identity は要らない。
//!
//! - ロック画面は WinRT `TrySetLockScreenImageAsync` がパッケージ化したアプリ向けのため範囲外
//!   （ファイルも書かず OS も呼ばずに `Unsupported`）。WinRT `UserProfilePersonalizationSettings`
//!   をホームにも使わないのは、パッケージのローカルフォルダのファイルを要するため
//! - 元ファイルは `fandhe_device::wallpaper::write_source_png_default` が書く（消さない）
//! - HRESULT・パスはエラーに載せない（security.md A09）
//!
//! # 既知の天井
//!
//! 公式は「エラーが無い限り TRUE」とだけ書く。グループポリシー等で反映されない場合は検知
//! できない（守らない範囲）。実機確認は #964。ローカルに MSVC が無いため型検査は
//! `x86_64-pc-windows-gnu` で行う。

use fandhe_device::photo_library::PngImage;
use fandhe_device::wallpaper::{Wallpaper, WallpaperTarget};
use fandhe_device::DeviceError;
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER,
};

/// Windows の `Wallpaper` 実装。
pub(crate) struct WindowsWallpaper;

impl Wallpaper for WindowsWallpaper {
    #[allow(unsafe_code)]
    fn set_wallpaper(
        &self,
        image: &PngImage,
        target: WallpaperTarget,
    ) -> Result<WallpaperTarget, DeviceError> {
        use std::os::windows::ffi::OsStrExt;

        if target == WallpaperTarget::Lock {
            return Err(DeviceError::Unsupported("wallpaper"));
        }
        let path = fandhe_device::wallpaper::write_source_png_default(image)?;
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(DeviceError::Internal("wallpaper path"));
        }
        wide.push(0);
        // SAFETY: `wide` は NUL 終端の有効な UTF-16 で、この呼び出しの間だけ生きていればよい
        // （OS は文字列を保持せず、レジストリへ複製する）。`SPI_SETDESKWALLPAPER` の pvParam は
        // 書き換えられない文字列として扱われる。
        let result = unsafe {
            SystemParametersInfoW(
                SPI_SETDESKWALLPAPER,
                0,
                Some(wide.as_mut_ptr().cast()),
                SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
            )
        };
        match result {
            Ok(()) => Ok(WallpaperTarget::Home),
            Err(_) => Err(DeviceError::Internal("wallpaper set")),
        }
    }
}
