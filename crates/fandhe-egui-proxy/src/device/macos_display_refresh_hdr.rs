//! macOS 向け画面のリフレッシュレート・HDR・広色域実装（Issue #870）。
//! `macos/fandhe_macos_display_refresh_hdr.m`（`NSScreen` / `CGDisplayMode`、`build.rs` が macOS
//! ターゲットの feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。
//! `unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/display_refresh_hdr.rs`
//! の macOS 経路から呼ばれ、契約は `fandhe_device::display_refresh_hdr`。
//!
//! `.m` が返す生値（-1 の番兵、headroom）は [`from_raw`] で契約の値へ写す。写像はホストでも
//! テストできるようモジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 窓が複数画面にまたがるときは `NSWindow.screen`（OS が返す主な画面）のとおり
//! - 今の Hz は `CGDisplayModeGetRefreshRate` で、0 を返す画面と可変レート（ProMotion）の
//!   瞬間値は「取れない」または公称値になる
//! - 実機の目視確認は #868

#[cfg(target_os = "macos")]
use fandhe_device::display_refresh_hdr::require_any;
use fandhe_device::display_refresh_hdr::{hdr_from_headroom, hz_from_raw, DisplayCapabilities};

/// `.m` の `FandheDisplayRaw` と同じレイアウト。
#[repr(C)]
#[derive(Clone, Copy)]
struct Raw {
    max_fps: f64,
    current_hz: f64,
    headroom: f64,
    p3: i32,
}

/// `.m` の生値を契約の値へ写す。-1・0・非有限の Hz は `None`、p3 は 0 / 1 以外を `None`。
fn from_raw(raw: Raw) -> DisplayCapabilities {
    DisplayCapabilities {
        current_hz: hz_from_raw(raw.current_hz),
        max_hz: hz_from_raw(raw.max_fps),
        hdr: hdr_from_headroom(raw.headroom),
        wide_gamut: match raw.p3 {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_display_refresh_hdr.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_display_refresh_hdr_read(out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_macos_display_refresh_hdr_read(out) }
    }
}

#[cfg(target_os = "macos")]
struct MacDisplayRefreshHdr;

#[cfg(target_os = "macos")]
impl fandhe_device::display_refresh_hdr::DisplayRefreshHdr for MacDisplayRefreshHdr {
    fn read(&self) -> Result<DisplayCapabilities, fandhe_device::DeviceError> {
        let mut raw = Raw {
            max_fps: -1.0,
            current_hz: -1.0,
            headroom: f64::NAN,
            p3: -1,
        };
        if ffi::read(&mut raw) != 0 {
            return Err(fandhe_device::DeviceError::Internal(
                "display_refresh_hdr read",
            ));
        }
        require_any(from_raw(raw))
    }
}

/// macOS の `DisplayRefreshHdr` 実装を返す（`device/display_refresh_hdr.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::display_refresh_hdr::DisplayRefreshHdr> {
    Box::new(MacDisplayRefreshHdr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> Raw {
        Raw {
            max_fps: 120.0,
            current_hz: 59.94,
            headroom: 2.0,
            p3: 1,
        }
    }

    /// テストスレッドは main ではないため、窓が無い状態は成功にならず `Internal` になる。
    #[cfg(target_os = "macos")]
    #[test]
    fn real_read_off_main_thread_is_internal() {
        assert!(matches!(
            implementation().read(),
            Err(fandhe_device::DeviceError::Internal(
                "display_refresh_hdr read"
            ))
        ));
    }

    #[test]
    fn maps_normal_values() {
        let c = from_raw(raw());
        assert_eq!(c.current_hz, Some(60));
        assert_eq!(c.max_hz, Some(120));
        assert_eq!(c.hdr, Some(true));
        assert_eq!(c.wide_gamut, Some(true));
    }

    #[test]
    fn sentinels_are_none() {
        let mut r = raw();
        r.max_fps = -1.0;
        r.current_hz = 0.0;
        r.headroom = f64::NAN;
        r.p3 = 2;
        assert_eq!(from_raw(r), DisplayCapabilities::default());
        r.current_hz = f64::NAN;
        assert_eq!(from_raw(r).current_hz, None);
    }

    #[test]
    fn low_headroom_is_no_not_unknown() {
        let mut r = raw();
        r.headroom = 1.0;
        r.p3 = 0;
        let c = from_raw(r);
        assert_eq!(c.hdr, Some(false));
        assert_eq!(c.wide_gamut, Some(false));
    }
}
