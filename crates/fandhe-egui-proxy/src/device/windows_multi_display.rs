//! Windows 向け複数ディスプレイ実装（Issue #1341）。`device/multi_display.rs` の Windows 経路から
//! 呼ばれ、Win32 の `EnumDisplayMonitors` / `GetMonitorInfoW` / `GetDpiForMonitor` を UI スレッドから
//! 同期で呼ぶ（package identity・管理者権限は不要。WinAppSDK `DisplayArea` は使わない）。
//! 契約は `fandhe_device::multi_display`。
//!
//! - サイズ: `rcMonitor` の差。winit が per-monitor v2 DPI aware にするため物理ピクセルの値
//! - 倍率: `GetDpiForMonitor(MDT_EFFECTIVE_DPI)` / 96。失敗しても画面は落とさず `None`
//! - 主画面: `MONITORINFOF_PRIMARY`。名前は取らない（`szDevice` は内部名で、友好名は範囲外）
//!
//! 個体識別子（EDID・デバイスパス）は読まない。値の写像は純関数に切り出し、ホストでもテストする。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enumdisplaymonitors>
//!
//! # 既知の天井
//!
//! - 擬似モニター（リモート・ミラードライバ）は OS が返すとおり。`SM_CMONITORS` との照合はしない
//! - 画面の位置・並びは扱わない
//! - 実機確認は #1340。ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

/// `rcMonitor` の矩形から (幅, 高さ) を求める。負の座標（主画面の左にある画面）を許し、
/// 差は checked で取る。0 以下・溢れは `None`。
fn rect_size(left: i32, top: i32, right: i32, bottom: i32) -> Option<(u32, u32)> {
    let w = right.checked_sub(left)?;
    let h = bottom.checked_sub(top)?;
    Some((
        u32::try_from(w).ok().filter(|&n| n > 0)?,
        u32::try_from(h).ok().filter(|&n| n > 0)?,
    ))
}

/// 有効 DPI を倍率にする（96 dpi = 1.0）。0 は取れなかったとみなして `None`。
fn scale_from_dpi(dpi: u32) -> Option<f32> {
    (dpi != 0).then(|| dpi as f32 / 96.0)
}

/// `MONITORINFO.dwFlags` が主画面を示すか（`MONITORINFOF_PRIMARY` = 1）。
fn is_primary_flag(flags: u32) -> bool {
    flags & 1 != 0
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod win {
    use super::{is_primary_flag, rect_size, scale_from_dpi};
    use fandhe_device::multi_display::{MultiDisplay, ScreenInfo};
    use fandhe_device::DeviceError;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

    pub(super) struct WindowsMultiDisplay;

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` は `monitors()` が渡した `&mut Vec<HMONITOR>` で、コールバックは
        // `EnumDisplayMonitors` の呼び出し中に同期で呼ばれるため参照先は生存し他に別名が無い。
        let v = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        v.push(monitor);
        BOOL(1)
    }

    fn monitors() -> Option<Vec<HMONITOR>> {
        let mut v: Vec<HMONITOR> = Vec::new();
        // SAFETY: `v` は呼び出しの間生存する排他参照で、`collect` だけがこれを `LPARAM` 経由で
        // 触る。HDC・クリップ矩形は渡さない（全モニター）。
        let ok = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(collect),
                LPARAM(&mut v as *mut Vec<HMONITOR> as isize),
            )
        };
        ok.as_bool().then_some(v)
    }

    fn info(m: HMONITOR) -> Option<MONITORINFO> {
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `mi` は有効なスタック上の構造体で、`cbSize` を設定済み。関数は保持しない。
        unsafe { GetMonitorInfoW(m, &mut mi) }
            .as_bool()
            .then_some(mi)
    }

    fn scale(m: HMONITOR) -> Option<f32> {
        let (mut x, mut y) = (0u32, 0u32);
        // SAFETY: `x` / `y` は呼び出しの間有効な書き込み先。失敗は倍率不明として扱う。
        unsafe { GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y) }.ok()?;
        scale_from_dpi(x)
    }

    impl MultiDisplay for WindowsMultiDisplay {
        fn read(&self) -> Result<Vec<ScreenInfo>, DeviceError> {
            const FAILED: DeviceError = DeviceError::Internal("multi_display read");
            let mut out = Vec::new();
            for m in monitors().ok_or(FAILED)? {
                let mi = info(m).ok_or(FAILED)?;
                let r = mi.rcMonitor;
                let (width_px, height_px) =
                    rect_size(r.left, r.top, r.right, r.bottom).ok_or(FAILED)?;
                out.push(ScreenInfo {
                    width_px,
                    height_px,
                    scale: scale(m),
                    is_primary: is_primary_flag(mi.dwFlags),
                    name: None,
                });
            }
            Ok(out)
        }
    }
}

/// Windows の `MultiDisplay` 実装を返す（`device/multi_display.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::multi_display::MultiDisplay> {
    Box::new(win::WindowsMultiDisplay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_size_allows_negative_origin() {
        assert_eq!(rect_size(-1920, 0, 0, 1080), Some((1920, 1080)));
        assert_eq!(rect_size(0, 0, 2560, 1440), Some((2560, 1440)));
    }

    #[test]
    fn rect_size_rejects_empty_inverted_and_overflow() {
        assert_eq!(rect_size(0, 0, 0, 10), None);
        assert_eq!(rect_size(10, 0, 5, 10), None);
        assert_eq!(rect_size(i32::MIN, 0, i32::MAX, 10), None);
    }

    #[test]
    fn dpi_to_scale() {
        assert_eq!(scale_from_dpi(0), None);
        assert_eq!(scale_from_dpi(96), Some(1.0));
        assert_eq!(scale_from_dpi(144), Some(1.5));
    }

    #[test]
    fn primary_flag() {
        assert!(is_primary_flag(1));
        assert!(!is_primary_flag(0));
    }
}
