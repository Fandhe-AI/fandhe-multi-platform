//! Windows 向け画面のリフレッシュレート・HDR・広色域実装（Issue #870）。
//! `device/display_refresh_hdr.rs` の Windows 経路から呼ばれ、Win32 / DXGI を UI スレッドから
//! 同期で 1 回ずつ呼ぶ（package identity・管理者権限は不要）。契約は
//! `fandhe_device::display_refresh_hdr`。
//!
//! - 画面: メインウィンドウ（`windows_share::window()`）の `MonitorFromWindow(NEAREST)`
//! - 今の Hz と解像度: `EnumDisplaySettingsW(ENUM_CURRENT_SETTINGS)` の `dmDisplayFrequency` /
//!   `dmPelsWidth` / `dmPelsHeight`。0 と 1（ドライバ既定を表す値）は「取れない」
//! - 最大 Hz: 同関数のモード列挙のうち、今の解像度と同じモードの `dmDisplayFrequency` の最大
//! - HDR: 毎回作り直した DXGI factory の全出力から `DXGI_OUTPUT_DESC1.Monitor` が一致するものを探し、
//!   `ColorSpace` が `RGB_FULL_G2084_NONE_P2020` のときだけ `Some(true)`。オフと非対応は DXGI が
//!   区別できないため、それ以外と見つからない場合は `None`
//! - 広色域: DXGI で区別できない（SDR の広色域も `G22` で返る）ため常に `None`
//!
//! 画面の内部名（`szDevice`）は列挙の照合に使うだけで、返さず・ログに出さない。
//! `DisplayInformation.GetAdvancedColorInfo` は CoreWindow の無いデスクトップアプリでは使えない
//! ため使わない。値の写像は純関数に切り出し、ホストでもテストする。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-devmodew>、
//! <https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_6/ns-dxgi1_6-dxgi_output_desc1>
//!
//! # 既知の天井
//!
//! - 窓が複数画面にまたがるときは `MonitorFromWindow` が返す主な画面のとおり
//! - 可変リフレッシュレート（VRR）の瞬間値は取れない
//! - 実機確認は #868。ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

use fandhe_device::display_refresh_hdr::hz_from_raw;

/// `dmDisplayFrequency` を Hz にする。0 と 1 は「ハードウェア既定」を表す値で「取れない」。
fn hz_from_frequency(freq: u32) -> Option<u32> {
    if freq <= 1 {
        return None;
    }
    hz_from_raw(f64::from(freq))
}

/// モード列挙 `(幅, 高さ, Hz)` のうち今の解像度 `cur = (幅, 高さ)` と同じものの最大 Hz。
/// 一致するモードが無い・すべて「取れない」なら `None`。
fn max_hz_for(modes: impl Iterator<Item = (u32, u32, u32)>, cur: (u32, u32)) -> Option<u32> {
    modes
        .filter(|&(w, h, _)| (w, h) == cur)
        .filter_map(|(_, _, f)| hz_from_frequency(f))
        .max()
}

/// G2084 の色空間なら HDR 対応。それ以外は「取れない」（オフと非対応を区別できないため）。
fn hdr_from_g2084(is_g2084: bool) -> Option<bool> {
    is_g2084.then_some(true)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod win {
    use super::{hdr_from_g2084, hz_from_frequency, max_hz_for};
    use fandhe_device::display_refresh_hdr::{require_any, DisplayCapabilities, DisplayRefreshHdr};
    use fandhe_device::DeviceError;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW, ENUM_CURRENT_SETTINGS,
        ENUM_DISPLAY_SETTINGS_MODE, HMONITOR, MONITORINFO, MONITORINFOEXW,
        MONITOR_DEFAULTTONEAREST,
    };

    pub(super) struct WindowsDisplayRefreshHdr;

    fn devmode() -> DEVMODEW {
        DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        }
    }

    /// 画面の内部名（NUL 終端の UTF-16）を取る。呼び出し元は照合にだけ使う。
    fn device_name(hmon: HMONITOR) -> Option<[u16; 32]> {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `hmon` は直前に得た有効なハンドル。`MONITORINFOEXW` は先頭が `MONITORINFO` で、
        // `cbSize` に EX のサイズを設定済み（API が EX 版と判定する）。ポインタは保持されない。
        let ok = unsafe {
            GetMonitorInfoW(
                hmon,
                (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>(),
            )
        };
        ok.as_bool().then_some(info.szDevice)
    }

    /// 今の Hz と、今の解像度での最大 Hz。
    fn refresh(name: &[u16; 32]) -> (Option<u32>, Option<u32>) {
        let pname = PCWSTR(name.as_ptr());
        let mut cur = devmode();
        // SAFETY: `name` は NUL 終端でこの呼び出しの間生存し、`cur` は `dmSize` を設定した
        // 有効な構造体。関数はポインタを保持しない。
        if !unsafe { EnumDisplaySettingsW(pname, ENUM_CURRENT_SETTINGS, &mut cur) }.as_bool() {
            return (None, None);
        }
        let res = (cur.dmPelsWidth, cur.dmPelsHeight);
        let modes = (0u32..).map_while(|i| {
            let mut m = devmode();
            // SAFETY: 上と同じ。`i` はモード番号で、範囲外なら FALSE が返る。
            unsafe { EnumDisplaySettingsW(pname, ENUM_DISPLAY_SETTINGS_MODE(i), &mut m) }
                .as_bool()
                .then_some((m.dmPelsWidth, m.dmPelsHeight, m.dmDisplayFrequency))
        });
        (
            hz_from_frequency(cur.dmDisplayFrequency),
            max_hz_for(modes, res),
        )
    }

    /// `hmon` に対応する DXGI 出力の色空間が G2084 か。見つからない・失敗は `None`。
    fn hdr(hmon: HMONITOR) -> Option<bool> {
        // SAFETY: 引数なしで factory を作る。戻りは RAII の COM ポインタ（Drop で解放）。
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
        for a in 0u32.. {
            // SAFETY: `factory` は有効な COM ポインタ。範囲外は `DXGI_ERROR_NOT_FOUND` で終わる。
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(a) }) else {
                break;
            };
            for o in 0u32.. {
                // SAFETY: `adapter` は有効な COM ポインタ。範囲外は `DXGI_ERROR_NOT_FOUND` で終わる。
                let Ok(output) = (unsafe { adapter.EnumOutputs(o) }) else {
                    break;
                };
                let Ok(output6) = output.cast::<IDXGIOutput6>() else {
                    continue;
                };
                // SAFETY: `output6` は有効な COM ポインタ。戻りは値渡しの構造体。
                let Ok(desc) = (unsafe { output6.GetDesc1() }) else {
                    continue;
                };
                if desc.Monitor == hmon {
                    return hdr_from_g2084(
                        desc.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
                    );
                }
            }
        }
        None
    }

    impl DisplayRefreshHdr for WindowsDisplayRefreshHdr {
        fn read(&self) -> Result<DisplayCapabilities, DeviceError> {
            let fail = || DeviceError::Internal("display_refresh_hdr read");
            let hwnd = super::super::windows_share::window().ok_or_else(fail)?;
            // SAFETY: `hwnd` は `windows_share::window()` が返したメインウィンドウ。
            // 戻りのモニタハンドルはこの関数の中だけで使う。
            let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            if hmon.is_invalid() {
                return Err(fail());
            }
            let name = device_name(hmon).ok_or_else(fail)?;
            let (current_hz, max_hz) = refresh(&name);
            require_any(DisplayCapabilities {
                current_hz,
                max_hz,
                hdr: hdr(hmon),
                wide_gamut: None,
            })
        }
    }
}

/// Windows の `DisplayRefreshHdr` 実装を返す（`device/display_refresh_hdr.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::display_refresh_hdr::DisplayRefreshHdr> {
    Box::new(win::WindowsDisplayRefreshHdr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_zero_and_one_are_none() {
        assert_eq!(hz_from_frequency(0), None);
        assert_eq!(hz_from_frequency(1), None);
        assert_eq!(hz_from_frequency(60), Some(60));
    }

    #[test]
    fn max_hz_only_counts_current_resolution() {
        let modes = vec![
            (1920, 1080, 60),
            (1920, 1080, 144),
            (3840, 2160, 240),
            (1920, 1080, 1),
        ];
        assert_eq!(max_hz_for(modes.into_iter(), (1920, 1080)), Some(144));
    }

    #[test]
    fn max_hz_none_when_no_match_or_all_sentinel() {
        let modes = vec![(1280, 720, 60)];
        assert_eq!(max_hz_for(modes.into_iter(), (1920, 1080)), None);
        let sentinel = vec![(1920, 1080, 0), (1920, 1080, 1)];
        assert_eq!(max_hz_for(sentinel.into_iter(), (1920, 1080)), None);
    }

    #[test]
    fn hdr_only_known_when_g2084() {
        assert_eq!(hdr_from_g2084(true), Some(true));
        assert_eq!(hdr_from_g2084(false), None);
    }
}
