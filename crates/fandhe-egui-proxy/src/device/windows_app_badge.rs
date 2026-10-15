//! Windows 向けバッジ実装（Issue #1360、#29）。`ITaskbarList3::SetOverlayIcon` でメインウィンドウの
//! タスクバーボタンに件数（1〜9999）を数字入りの 16x16 アイコンとして重ねる（0 は外す）。
//! タスクバー進捗（`SetProgressValue`）は使わず、`set` は進捗を触らない（併用しない）。
//! HWND は `device/windows_share.rs::window()` と共通。`device/app_badge.rs` の Windows 経路から
//! 呼ばれ、契約は `fandhe_device::app_badge`。
//!
//! 入力の検証・HWND の有無の判定・アイコンのピクセル生成は純関数（[`plan`]・[`render`]・
//! [`description`]）にまとめ、ホストでもテストできるようモジュールは `cfg(any(windows, test))`。
//! `unsafe` は COM / `CreateIcon` 呼び出しに閉じ `// SAFETY:` を付ける。`HICON` は `OwnedIcon` の
//! `Drop` で全経路（成功・失敗・早期 return）で解放する。タスクバーはアイコンのコピーを持つため
//! `SetOverlayIcon` の後に解放してよい。
//!
//! # 既知の天井
//!
//! - アイコンは固定 16x16 px。高 DPI ではタスクバー側で拡大されてぼやける。問題になったら
//!   `GetSystemMetrics(SM_CXSMICON)` のサイズで描く
//! - `TaskbarButtonCreated` 前に呼んだ場合の挙動は範囲外（実機確認 #28）
//! - `HRESULT` と `CreateIcon` の失敗はすべて `Internal("app_badge set")` に写す。こちらから
//!   `CoInitializeEx` はしない（`CO_E_NOTINITIALIZED` も失敗として返す）

use fandhe_device::app_badge::BadgeValue;
use fandhe_device::DeviceError;

/// アイコンの一辺（px）。
const SIZE: usize = 16;

/// 実行する操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// オーバーレイを外す。
    Clear,
    /// 件数（1〜9999）を数字で重ねる。
    Overlay(u32),
}

/// HWND 判定と操作の決定。値の範囲検証は `BadgeValue::new` が OS 型へ変換する前に済ませている（契約表 5）。
pub(crate) fn plan(value: BadgeValue, has_window: bool) -> Result<Step, DeviceError> {
    if !has_window {
        return Err(DeviceError::Internal("app_badge window"));
    }
    Ok(if value.is_clear() {
        Step::Clear
    } else {
        Step::Overlay(value.get())
    })
}

/// `CreateIcon` に渡すビット列。
pub(crate) struct IconBits {
    /// 32bpp BGRA（上から下）。
    pub xor: [u8; SIZE * SIZE * 4],
    /// 1bpp マスク（1 = 透明）。1 行 16 bit = 2 byte。
    pub and: [u8; SIZE * SIZE / 8],
}

/// 3x5 の数字ドットフォント。各行の下位 3 bit を使う。
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// 件数を赤い丸＋白い数字で描く。不透明画素は alpha=0xFF・マスク 0、透明画素は alpha=0・
/// マスク 1（alpha が効く場合もマスクで判定される場合も同じ見た目になる）。
pub(crate) fn render(count: u32) -> IconBits {
    let digits: Vec<usize> = count
        .to_string()
        .bytes()
        .map(|b| usize::from(b - b'0'))
        .collect();
    let width = digits.len() * 4 - 1;
    let x0 = (SIZE - width) / 2;
    let y0 = (SIZE - 5) / 2;
    let mut white = [[false; SIZE]; SIZE];
    for (n, d) in digits.iter().enumerate() {
        for (row, bits) in DIGITS[*d].iter().enumerate() {
            for col in 0..3 {
                if bits >> (2 - col) & 1 == 1 {
                    white[y0 + row][x0 + n * 4 + col] = true;
                }
            }
        }
    }
    let mut out = IconBits {
        xor: [0; SIZE * SIZE * 4],
        and: [0xFF; SIZE * SIZE / 8],
    };
    let c = (SIZE as f32 - 1.0) / 2.0;
    #[allow(clippy::needless_range_loop)]
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            if dx * dx + dy * dy > 8.0 * 8.0 {
                continue;
            }
            let (b, g, r) = if white[y][x] {
                (0xFF, 0xFF, 0xFF)
            } else {
                (0x2D, 0x2D, 0xD6)
            };
            let o = (y * SIZE + x) * 4;
            out.xor[o..o + 4].copy_from_slice(&[b, g, r, 0xFF]);
            out.and[y * 2 + x / 8] &= !(0x80 >> (x % 8));
        }
    }
    out
}

/// アクセシビリティ用の説明文。入力は検証済みの数値だけで、利用者の文字列は入らない。
pub(crate) fn description(count: u32) -> String {
    format!("未読 {count} 件")
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod com {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, DestroyIcon, HICON};

    use super::{description, render, Step, SIZE};

    /// `CreateIcon` で作った `HICON` の所有者。`Drop` で必ず解放する（契約表 9）。
    struct OwnedIcon(HICON);

    impl Drop for OwnedIcon {
        fn drop(&mut self) {
            // SAFETY: `self.0` は `CreateIcon` が返した未解放のハンドルで、ここでだけ解放する。
            // 失敗しても打てる手がないため無視する。
            let _ = unsafe { DestroyIcon(self.0) };
        }
    }

    /// COM 呼び出しの失敗は呼び出し元で `Internal` に写す（OS の文言は出さない）。
    pub(super) fn apply(
        hwnd: windows::Win32::Foundation::HWND,
        step: Step,
    ) -> windows::core::Result<()> {
        // SAFETY: `TaskbarList`/`CLSCTX_INPROC_SERVER` は定数のみ。失敗は `Result` で返る。
        let list: ITaskbarList3 =
            unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }?;
        // SAFETY: `list` は直前に取得した有効な COM ポインタ（`Drop` で Release される）。
        unsafe { list.HrInit() }?;
        match step {
            // SAFETY: `hwnd` は `windows_share::window()` が返したメインウィンドウ。
            // 空の `HICON` と null の説明文はオーバーレイを外す指定。
            Step::Clear => unsafe { list.SetOverlayIcon(hwnd, HICON::default(), PCWSTR::null()) },
            Step::Overlay(n) => {
                let bits = render(n);
                // SAFETY: `xor` は 16x16x32bpp、`and` は 16x16x1bpp のバッファで、幅・高さ・
                // ビット数と配列長が定数 `SIZE` から一致する。`CreateIcon` は呼び出し中に
                // コピーするので、バッファは関数末尾まで生きていればよい。
                let icon = OwnedIcon(unsafe {
                    CreateIcon(
                        None,
                        SIZE as i32,
                        SIZE as i32,
                        1,
                        32,
                        bits.and.as_ptr(),
                        bits.xor.as_ptr(),
                    )
                }?);
                let text = HSTRING::from(description(n));
                // SAFETY: `hwnd` は同上。`icon.0` は有効な `HICON`、`text` は呼び出し中生きる。
                // タスクバーはアイコンを複製するので、戻り後に `icon` が解放されてよい。
                unsafe { list.SetOverlayIcon(hwnd, icon.0, &text) }
            }
        }
    }
}

#[cfg(target_os = "windows")]
struct WindowsAppBadge;

#[cfg(target_os = "windows")]
impl fandhe_device::app_badge::AppBadge for WindowsAppBadge {
    fn set(
        &self,
        value: BadgeValue,
    ) -> Result<fandhe_device::app_badge::BadgeOutcome, DeviceError> {
        let hwnd = super::windows_share::window();
        let step = plan(value, hwnd.is_some())?;
        let Some(hwnd) = hwnd else {
            return Err(DeviceError::Internal("app_badge window"));
        };
        com::apply(hwnd, step).map_err(|_| DeviceError::Internal("app_badge set"))?;
        Ok(fandhe_device::app_badge::BadgeOutcome::Submitted)
    }
}

/// Windows の `AppBadge` 実装を返す（`device/app_badge.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::app_badge::AppBadge> {
    Box::new(WindowsAppBadge)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(n: u32) -> BadgeValue {
        BadgeValue::new(n).unwrap()
    }

    #[test]
    fn plan_table() {
        assert_eq!(plan(v(0), true).unwrap(), Step::Clear);
        assert_eq!(plan(v(1), true).unwrap(), Step::Overlay(1));
        assert_eq!(plan(v(101), true).unwrap(), Step::Overlay(101));
        assert_eq!(plan(v(9999), true).unwrap(), Step::Overlay(9999));
        assert!(matches!(
            plan(v(5), false),
            Err(DeviceError::Internal("app_badge window"))
        ));
    }

    #[test]
    fn render_is_centered_and_consistent() {
        for n in [1, 42, 9999] {
            let b = render(n);
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let alpha = b.xor[(y * SIZE + x) * 4 + 3];
                    let masked = b.and[y * 2 + x / 8] & (0x80 >> (x % 8)) != 0;
                    assert_eq!(alpha == 0xFF, !masked, "n={n} ({x},{y})");
                }
            }
            // 角は円の外で透明。
            assert_eq!(b.xor[3], 0);
            // 白画素の左右余白差は 1 px 以内。
            let white: Vec<usize> = (0..SIZE * SIZE)
                .filter(|i| b.xor[i * 4..i * 4 + 3] == [0xFF; 3])
                .map(|i| i % SIZE)
                .collect();
            let (min, max) = (white.iter().min().unwrap(), white.iter().max().unwrap());
            // 数字の画素が円の外で欠けない（4 桁の左端で再発した）。
            let bits: u32 = n
                .to_string()
                .bytes()
                .map(|c| {
                    DIGITS[usize::from(c - b'0')]
                        .iter()
                        .map(|r| r.count_ones())
                        .sum::<u32>()
                })
                .sum();
            assert_eq!(white.len() as u32, bits, "n={n}");
            assert!((*min as i32 - (SIZE - 1 - *max) as i32).abs() <= 1, "n={n}");
        }
        assert!(render(1).xor != render(7).xor);
    }

    #[test]
    fn description_format() {
        assert_eq!(description(5), "未読 5 件");
    }
}
