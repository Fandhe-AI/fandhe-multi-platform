//! iOS 向け画面のリフレッシュレート・HDR・広色域実装（親 #177、Issue #871）。
//! `ios/fandhe_ios_display_refresh_hdr.m`（`UIScreen` / `UITraitCollection`、`build.rs` が iOS
//! ターゲットの feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。
//! `unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/display_refresh_hdr.rs`
//! の iOS 経路から呼ばれ、契約の SSOT は `fandhe_device::display_refresh_hdr`。
//!
//! `.m` が返す生値（-1 の番兵、headroom）は [`from_raw`] で契約の値へ写す。写像はホストでも
//! テストできるようモジュールは `cfg(any(ios, test))`。
//!
//! - 今の Hz は常に `None`。`CADisplayLink` の毎フレーム観測を要し、推定しない
//! - HDR は `potentialEDRHeadroom` が 1.0 を超えるか。`currentEDRHeadroom` は EDR を描いている間だけ
//!   1 を超えるので使わない
//! - 再描画: 本機能は main スレッドの同期読み取りで OS コールバックを持たない。完了は
//!   `CompletionSender` が送り、そこで `egui::Context::request_repaint` が呼ばれるため
//!   `crate::ios::request_repaint` は不要
//!
//! # 既知の天井
//!
//! - 複数画面のときは OS が返す窓の画面（`window.windowScene.screen`）のとおり
//! - ProMotion の瞬間値は取らない。`maximumFramesPerSecond` の実機での値は #868 で確かめる

#[cfg(target_os = "ios")]
use fandhe_device::display_refresh_hdr::require_any;
use fandhe_device::display_refresh_hdr::{hdr_from_headroom, hz_from_raw, DisplayCapabilities};

/// `.m` の `FandheIosDisplayRaw` と同じレイアウト。
#[repr(C)]
#[derive(Clone, Copy)]
struct Raw {
    max_fps: f64,
    headroom: f64,
    p3: i32,
}

/// `.m` の生値を契約の値へ写す。-1・0・非有限の Hz は `None`、p3 は 0 / 1 以外を `None`。
/// 今の Hz は常に `None`。
fn from_raw(raw: Raw) -> DisplayCapabilities {
    DisplayCapabilities {
        current_hz: None,
        max_hz: hz_from_raw(raw.max_fps),
        hdr: hdr_from_headroom(raw.headroom),
        wide_gamut: match raw.p3 {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
    }
}

#[cfg(target_os = "ios")]
mod ffi {
    use super::Raw;

    // SAFETY: `ios/fandhe_ios_display_refresh_hdr.m`（`build.rs` が iOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_display_refresh_hdr_read(out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_ios_display_refresh_hdr_read(out) }
    }
}

#[cfg(target_os = "ios")]
struct IosDisplayRefreshHdr;

#[cfg(target_os = "ios")]
impl fandhe_device::display_refresh_hdr::DisplayRefreshHdr for IosDisplayRefreshHdr {
    fn read(&self) -> Result<DisplayCapabilities, fandhe_device::DeviceError> {
        let mut raw = Raw {
            max_fps: -1.0,
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

/// iOS の `DisplayRefreshHdr` 実装を返す（`device/display_refresh_hdr.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::display_refresh_hdr::DisplayRefreshHdr> {
    Box::new(IosDisplayRefreshHdr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> Raw {
        Raw {
            max_fps: 120.0,
            headroom: 2.0,
            p3: 1,
        }
    }

    #[test]
    fn maps_normal_values() {
        let c = from_raw(raw());
        assert_eq!(c.current_hz, None);
        assert_eq!(c.max_hz, Some(120));
        assert_eq!(c.hdr, Some(true));
        assert_eq!(c.wide_gamut, Some(true));
    }

    #[test]
    fn sentinels_are_none() {
        let mut r = raw();
        r.max_fps = -1.0;
        r.headroom = f64::NAN;
        r.p3 = -1;
        assert_eq!(from_raw(r), DisplayCapabilities::default());
        r.max_fps = 0.0;
        r.p3 = 2;
        assert_eq!(from_raw(r), DisplayCapabilities::default());
        r.max_fps = f64::NAN;
        assert_eq!(from_raw(r).max_hz, None);
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
