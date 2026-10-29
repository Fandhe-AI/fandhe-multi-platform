//! macOS 向け壁紙設定実装（Issue #970、親 #188）。`macos/fandhe_macos_wallpaper.m`
//! （`build.rs` が macOS ターゲットの feature `device` 有効時にコンパイルする）を
//! `extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/wallpaper.rs` の macOS 経路から main スレッドで呼ばれ、契約は
//! `fandhe_device::wallpaper`。
//!
//! ロック画面は公開 API が無いため、ファイルも書かず OS も呼ばずに `Unsupported` を返す。
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - Space ごとの壁紙は扱わない（現在の Space の設定に依存する）
//! - 実際に壁紙が変わったかの Mac 目視（複数画面・失敗系）は #964

#[cfg(target_os = "macos")]
use fandhe_device::photo_library::PngImage;
use fandhe_device::wallpaper::WallpaperTarget;
use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
pub(crate) fn outcome_from_code(code: i32) -> Result<WallpaperTarget, DeviceError> {
    match code {
        0 => Ok(WallpaperTarget::Home),
        2 => Err(DeviceError::Internal("wallpaper not on main thread")),
        3 => Err(DeviceError::Internal("wallpaper set")),
        4 => Err(DeviceError::Internal("wallpaper readback")),
        5 => Err(DeviceError::Internal("wallpaper no screen")),
        6 => Err(DeviceError::Internal("wallpaper path")),
        _ => Err(DeviceError::Internal("wallpaper")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_wallpaper.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_wallpaper_set(path_utf8: *const std::ffi::c_char) -> i32;
    }

    /// 結果コードを返す（写像は [`super::outcome_from_code`]）。
    #[allow(unsafe_code)]
    pub(super) fn set(path: &std::ffi::CStr) -> i32 {
        // SAFETY: `path` は NUL 終端の有効な C 文字列で、呼び出しの間だけ生きる。`.m` はその場で
        // `NSURL` へ複製し、ポインタを保持しない。main 以外なら `.m` が AppKit に触らず 2 を返す。
        unsafe { fandhe_macos_wallpaper_set(path.as_ptr()) }
    }
}

/// macOS の `Wallpaper` 実装（`device/wallpaper.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) struct MacosWallpaper;

#[cfg(target_os = "macos")]
impl fandhe_device::wallpaper::Wallpaper for MacosWallpaper {
    fn set_wallpaper(
        &self,
        image: &PngImage,
        target: WallpaperTarget,
    ) -> Result<WallpaperTarget, DeviceError> {
        if target == WallpaperTarget::Lock {
            return Err(DeviceError::Unsupported("wallpaper"));
        }
        let path = fandhe_device::wallpaper::write_source_png_default(image)?;
        let c_path = path
            .to_str()
            .and_then(|s| std::ffi::CString::new(s).ok())
            .ok_or(DeviceError::Internal("wallpaper path"))?;
        outcome_from_code(ffi::set(&c_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert!(matches!(outcome_from_code(0), Ok(WallpaperTarget::Home)));
        let expected = [
            (2, "wallpaper not on main thread"),
            (3, "wallpaper set"),
            (4, "wallpaper readback"),
            (5, "wallpaper no screen"),
            (6, "wallpaper path"),
            (1, "wallpaper"),
            (7, "wallpaper"),
            (-1, "wallpaper"),
            (i32::MAX, "wallpaper"),
        ];
        for (code, msg) in expected {
            assert!(
                matches!(outcome_from_code(code), Err(DeviceError::Internal(m)) if m == msg),
                "code {code}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn lock_is_unsupported_without_touching_os() {
        use fandhe_device::wallpaper::Wallpaper;
        let mut b = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        b.push(0);
        let png = PngImage::new(b).expect("valid signature");
        assert!(matches!(
            MacosWallpaper.set_wallpaper(&png, WallpaperTarget::Lock),
            Err(DeviceError::Unsupported("wallpaper"))
        ));
    }
}
