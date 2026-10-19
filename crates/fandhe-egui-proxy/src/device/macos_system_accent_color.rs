//! macOS 向けシステムのアクセント色実装（Issue #421、親 #103）。
//! `macos/fandhe_macos_system_accent_color.m`（`NSColor.controlAccentColor`、`build.rs` が macOS
//! ターゲットの feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。
//! `unsafe` はこの呼び出しに閉じる。`device/system_accent_color.rs` の macOS 経路から呼ばれ、
//! 契約は `fandhe_device::system_accent_color`。戻り値の写像は同モジュールの `rgb_from_code`（SSOT）。
//!
//! # 既知の天井
//!
//! - 「マルチカラー」設定のときに返る青を本物と区別しない
//! - 外観（ライト / ダーク）に合わせて解決した値で、OS 設定画面の見た目と完全には一致しない
//! - 実機の目視確認は #415

use fandhe_device::system_accent_color::{rgb_from_code, AccentColorSource, Rgb};
use fandhe_device::DeviceError;

// SAFETY: `macos/fandhe_macos_system_accent_color.m` が定義する C ABI。引数なし・戻り値 i32 のみで、
// ポインタを受け渡さない。main スレッド以外・NSApp 未確立では `.m` 側が -1 を返す。
#[allow(unsafe_code)]
extern "C" {
    fn fandhe_macos_system_accent_color_read() -> i32;
}

/// [`AccentColorSource`] の macOS 実装。状態を持たない。
pub(crate) struct MacosAccentColor;

impl AccentColorSource for MacosAccentColor {
    fn read(&self) -> Result<Rgb, DeviceError> {
        // SAFETY: 上記 extern 宣言のとおり引数なしの純粋な読み取り。
        #[allow(unsafe_code)]
        let code = unsafe { fandhe_macos_system_accent_color_read() };
        rgb_from_code(code)
    }
}
