//! macOS 向け複数ディスプレイ実装（Issue #1341）。`macos/fandhe_macos_multi_display.m`
//! （`NSScreen.screens`、`build.rs` が macOS ターゲットの feature `device` 有効時にコンパイルする）
//! を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を
//! 明記する。`device/multi_display.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::multi_display`。
//!
//! `.m` が返す生値（固定長の名前バッファ）は [`from_raw`] で契約の値へ写す。写像はホストでも
//! テストできるようモジュールは `cfg(any(macos, test))`。`.m` は main thread 以外では失敗を返す
//! （`cargo test` のワーカースレッドで実 FFI を呼ぶと `Internal` になる。成功系は #1340 の Mac 目視）。
//!
//! # 既知の天井
//!
//! - 画面数の上限は [`CAP`]。超えたら先頭だけを返さず失敗にする
//! - ミラーリング中の画面は `NSScreen.screens` が返すとおり（物理数と一致するとは限らない）
//! - 個体識別子は読まない

use std::ffi::CStr;

use fandhe_device::multi_display::ScreenInfo;

/// `.m` に渡す出力バッファの画面数上限。
const CAP: usize = 16;

/// `.m` の `FandheScreenRaw` と同じレイアウト。
#[repr(C)]
#[derive(Clone, Copy)]
struct Raw {
    width_px: u32,
    height_px: u32,
    scale: f64,
    is_primary: u8,
    name: [u8; 128],
}

/// `.m` の生値を契約の値へ写す。名前は NUL 終端の UTF-8 で、不正なら `None`。
/// 倍率・主画面の正規化は `validate` が行う。
fn from_raw(rows: &[Raw]) -> Vec<ScreenInfo> {
    rows.iter()
        .map(|r| ScreenInfo {
            width_px: r.width_px,
            height_px: r.height_px,
            scale: Some(r.scale as f32),
            is_primary: r.is_primary != 0,
            name: CStr::from_bytes_until_nul(&r.name)
                .ok()
                .and_then(|c| c.to_str().ok())
                .map(str::to_owned),
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_multi_display.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_multi_display_read(out: *mut Raw, cap: u32, count: *mut u32) -> i32;
    }

    /// 0 = 成功。成功時は先頭 `count` 件が有効。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut [Raw], count: &mut u32) -> i32 {
        // SAFETY: `out` は `out.len()` 件分の確保済みで排他的な領域、`cap` はその件数。
        // `count` は有効な書き込み先。`.m` はポインタを保持しない。
        unsafe { fandhe_macos_multi_display_read(out.as_mut_ptr(), out.len() as u32, count) }
    }
}

#[cfg(target_os = "macos")]
struct MacMultiDisplay;

#[cfg(target_os = "macos")]
impl fandhe_device::multi_display::MultiDisplay for MacMultiDisplay {
    fn read(&self) -> Result<Vec<ScreenInfo>, fandhe_device::DeviceError> {
        let mut buf = [Raw {
            width_px: 0,
            height_px: 0,
            scale: 0.0,
            is_primary: 0,
            name: [0; 128],
        }; CAP];
        let mut count = 0u32;
        if ffi::read(&mut buf, &mut count) != 0 || count as usize > CAP {
            return Err(fandhe_device::DeviceError::Internal("multi_display read"));
        }
        Ok(from_raw(&buf[..count as usize]))
    }
}

/// macOS の `MultiDisplay` 実装を返す（`device/multi_display.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::multi_display::MultiDisplay> {
    Box::new(MacMultiDisplay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &[u8]) -> Raw {
        let mut buf = [0u8; 128];
        buf[..name.len()].copy_from_slice(name);
        Raw {
            width_px: 3024,
            height_px: 1964,
            scale: 2.0,
            is_primary: 1,
            name: buf,
        }
    }

    /// main thread 以外では実 FFI が失敗を返す（`dispatch_sync` せずガードする）。
    #[cfg(target_os = "macos")]
    #[test]
    fn off_main_thread_is_internal_error() {
        let r = implementation().read();
        assert!(matches!(r, Err(fandhe_device::DeviceError::Internal(_))));
    }

    #[test]
    fn maps_name_and_primary() {
        let v = from_raw(&[raw(b"Studio Display"), raw(b"")]);
        assert_eq!(v[0].name.as_deref(), Some("Studio Display"));
        assert!(v[0].is_primary);
        assert_eq!(v[0].scale, Some(2.0));
        assert_eq!(v[1].name.as_deref(), Some(""));
    }

    #[test]
    fn invalid_utf8_or_missing_nul_is_none() {
        assert_eq!(from_raw(&[raw(&[0xff, 0xfe])])[0].name, None);
        let mut r = raw(b"");
        r.name = [b'a'; 128];
        assert_eq!(from_raw(&[r])[0].name, None);
    }
}
