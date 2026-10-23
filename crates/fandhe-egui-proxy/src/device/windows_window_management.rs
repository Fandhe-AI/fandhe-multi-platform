//! Windows 向けウィンドウ管理の読み取り（Issue #637、親 #127）。
//! `device/window_management.rs::implementation` の Windows 経路から呼ばれ、
//! [`NativeReads`] の 2 関数（最前面か・今いる画面の作業領域）だけを Win32 で読む。
//! 変更（位置・大きさ・最前面）は `ViewportWindowManager` が eframe の `ViewportCommand` で行う
//! （`SetWindowPos` を直接呼ぶと winit が持つフラグと食い違うため）。
//! 契約は `fandhe_device::window_management`。
//!
//! - 最前面: `GetWindowLongW(GWL_EXSTYLE)` の `WS_EX_TOPMOST`。失敗時の 0 と正常な 0 は
//!   `SetLastError(0)` と `GetLastError` で見分け、読めなければ `None`（`false` で代用しない）
//! - 作業領域: `MonitorFromWindow` + `GetMonitorInfoW` の `rcWork`。winit が per-monitor v2 DPI
//!   aware にするため物理 px で、`pixels_per_point` で割って egui point にする
//!   （`snapshot().outer` と同じ座標系）
//! - HWND は `windows_share::window()`。`unsafe` は呼び出し 1 つずつに閉じ `// SAFETY:` を付ける
//! - UI スレッドから同期で呼ぶ。OS のエラーコード・座標・画面の大きさはログにも画面にも出さない
//!
//! # 既知の天井
//!
//! - 外枠に見えない縁が数 px あるため、左上へ寄せた見た目は数 px 内側になる
//! - 倍率の違う画面をまたぐ移動は守備範囲外
//! - 1 プロセス 1 ウィンドウが前提（`windows_share` の static）
//! - MSVC ビルドと実機は未検証（確認用 #629）
//!
//! 一次情報: learn.microsoft.com の `GetWindowLongW`・`GetMonitorInfoW`・`MonitorFromWindow`。

use fandhe_device::window_management::WindowFrame;

/// `WS_EX_TOPMOST`。ホストのテストでも使うため定数で持ち、Windows ビルドで `windows` クレートの値と照合する。
const WS_EX_TOPMOST_BIT: u32 = 0x0000_0008;

#[cfg(target_os = "windows")]
const _: () = assert!(
    WS_EX_TOPMOST_BIT == windows::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST.0,
    "WS_EX_TOPMOST の値が windows クレートと食い違っている"
);

/// `rcWork`（物理 px）を egui point の [`WindowFrame`] に換算する。
/// 検査は換算の前に行う。倍率が非有限・0 以下、幅・高さが 0 以下・溢れなら `None`。
/// 負の原点（主画面の左や上にある副画面）は許す。
fn work_area_from_rect(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    pixels_per_point: f32,
) -> Option<WindowFrame> {
    let ppp = f64::from(pixels_per_point);
    if !ppp.is_finite() || ppp <= 0.0 {
        return None;
    }
    let width = right.checked_sub(left).filter(|w| *w > 0)?;
    let height = bottom.checked_sub(top).filter(|h| *h > 0)?;
    let frame = WindowFrame {
        x: f64::from(left) / ppp,
        y: f64::from(top) / ppp,
        width: f64::from(width) / ppp,
        height: f64::from(height) / ppp,
    };
    [frame.x, frame.y, frame.width, frame.height]
        .iter()
        .all(|v| v.is_finite())
        .then_some(frame)
}

/// `GetWindowLongW(GWL_EXSTYLE)` の戻り値から最前面かを求める。
/// 戻り値 0 かつ `last_error` 非 0 は呼び出し失敗で `None`。値が非 0 なら成功とみなす。
fn topmost_from_exstyle(value: i32, last_error: u32) -> Option<bool> {
    if value == 0 && last_error != 0 {
        return None;
    }
    Some((value as u32) & WS_EX_TOPMOST_BIT != 0)
}

#[cfg(target_os = "windows")]
mod native {
    use std::sync::Arc;

    use fandhe_device::window_management::{WindowFrame, WindowManager};
    use windows::Win32::Foundation::{GetLastError, SetLastError, WIN32_ERROR};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GWL_EXSTYLE};

    use super::super::window_management::{NativeReads, ViewportWindowManager};
    use super::{topmost_from_exstyle, work_area_from_rect};

    /// [`NativeReads`] の Windows 実装。状態を持たない。
    pub(crate) struct WindowsReads;

    impl NativeReads for WindowsReads {
        #[allow(unsafe_code)]
        fn always_on_top(&self) -> Option<bool> {
            let hwnd = super::super::windows_share::window()?;
            // SAFETY: 引数なしでスレッドのエラー値を 0 にするだけ。UI スレッドから同期で呼ぶ。
            unsafe { SetLastError(WIN32_ERROR(0)) };
            // SAFETY: `hwnd` は `windows_share::window()`（`remember_window` が控えた HWND、
            // `ProxyApp` の生存中はウィンドウも生存する）経由の有効なハンドル。値を返すだけで
            // ポインタを保持しない。
            let value = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
            // SAFETY: 引数なしで直前の呼び出しのエラー値を読むだけ（同一スレッド）。
            let last_error = unsafe { GetLastError() }.0;
            topmost_from_exstyle(value, last_error)
        }

        #[allow(unsafe_code)]
        fn work_area(&self, ctx: &egui::Context) -> Option<WindowFrame> {
            let hwnd = super::super::windows_share::window()?;
            // SAFETY: `hwnd` は有効なウィンドウハンドル（`always_on_top` と同じ根拠）。
            // 戻りのモニタハンドルは保持しない。
            let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            if monitor.is_invalid() {
                return None;
            }
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: `monitor` は直前に得た有効なハンドル。出力先 `info` はこの呼び出しの間だけ
            // 有効なスタック上の構造体で、`cbSize` を設定済み。関数はポインタを保持しない。
            if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
                return None;
            }
            let r = info.rcWork;
            work_area_from_rect(r.left, r.top, r.right, r.bottom, ctx.pixels_per_point())
        }
    }

    /// Windows の `WindowManager` を返す。
    pub(crate) fn implementation(ctx: egui::Context) -> Arc<dyn WindowManager> {
        Arc::new(ViewportWindowManager {
            ctx,
            reads: WindowsReads,
        })
    }
}

#[cfg(target_os = "windows")]
pub(crate) use native::implementation;

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f64, y: f64, w: f64, h: f64) -> WindowFrame {
        WindowFrame {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn 倍率100はそのまま() {
        assert_eq!(
            work_area_from_rect(0, 0, 1920, 1040, 1.0),
            Some(frame(0.0, 0.0, 1920.0, 1040.0))
        );
    }

    #[test]
    fn 倍率150はpointへ割る() {
        assert_eq!(
            work_area_from_rect(0, 0, 2880, 1560, 1.5),
            Some(frame(0.0, 0.0, 1920.0, 1040.0))
        );
    }

    #[test]
    fn zoom込みの割り切れない倍率() {
        let f = work_area_from_rect(0, 0, 1000, 500, 1.25).expect("有効");
        assert!((f.width - 800.0).abs() < 1e-9 && (f.height - 400.0).abs() < 1e-9);
    }

    #[test]
    fn 負の原点を許す() {
        assert_eq!(
            work_area_from_rect(-1920, 0, 0, 1040, 1.0),
            Some(frame(-1920.0, 0.0, 1920.0, 1040.0))
        );
    }

    #[test]
    fn 大きさ0と逆転は読めない() {
        assert_eq!(work_area_from_rect(0, 0, 0, 100, 1.0), None);
        assert_eq!(work_area_from_rect(0, 0, 100, 0, 1.0), None);
        assert_eq!(work_area_from_rect(100, 0, 0, 100, 1.0), None);
        assert_eq!(work_area_from_rect(0, 100, 100, 0, 1.0), None);
    }

    #[test]
    fn 溢れは読めない() {
        assert_eq!(work_area_from_rect(i32::MIN, 0, i32::MAX, 100, 1.0), None);
        assert_eq!(work_area_from_rect(0, i32::MIN, 100, i32::MAX, 1.0), None);
    }

    #[test]
    fn 不正な倍率は読めない() {
        for ppp in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(work_area_from_rect(0, 0, 100, 100, ppp), None);
        }
    }

    #[test]
    fn 最前面ビットを読む() {
        assert_eq!(topmost_from_exstyle(0x0000_0108, 0), Some(true));
        assert_eq!(topmost_from_exstyle(0x0000_0100, 0), Some(false));
    }

    #[test]
    fn 値0はエラー値で正常と失敗を分ける() {
        assert_eq!(topmost_from_exstyle(0, 0), Some(false));
        assert_eq!(topmost_from_exstyle(0, 5), None);
    }

    #[test]
    fn 値があればエラー値の残りは無視する() {
        assert_eq!(topmost_from_exstyle(0x8, 5), Some(true));
        assert_eq!(topmost_from_exstyle(0x100, 5), Some(false));
    }
}
