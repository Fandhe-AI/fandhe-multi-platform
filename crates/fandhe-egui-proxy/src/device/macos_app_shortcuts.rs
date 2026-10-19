//! macOS 向けアプリショートカット実装（Issue #424、親 #101）。Dock メニューに「デモを開く」を出す。
//! `macos/fandhe_macos_app_shortcuts.m`（`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じる。
//! `device/app_shortcuts.rs::platform` の macOS 経路から main thread で呼ばれ、契約は
//! `fandhe_device::app_shortcuts`。メニュー項目が選ばれると `.m` が既存の
//! `fandhe_rust_url_scheme_received`（`apple_url_scheme.rs`）へ固定 URL を渡し、`UrlInbox` で検査される。
//!
//! # 既知の天井
//!
//! - Dock メニューは起動中だけ出る。他アプリが同じ URL を送っても区別しない
//! - winit 0.30.13 の `WinitApplicationDelegate` は `applicationDockMenu:` を実装していない
//!   前提で、そのクラスへメソッドを足す。既にある場合は上書きせず失敗として返す
//! - 実機の目視（AppKit が Dock 右クリックのたびに selector を引くか）は #413

use fandhe_device::app_shortcuts::{AppShortcuts, Registration};
use fandhe_device::DeviceError;

/// `.m` の戻り値を契約上の結果へ写像する（SSOT）。
/// 0: 登録した / -1: main 以外か NSApp・delegate が無い / -2: 文字列を作れない /
/// -3: delegate が既に `applicationDockMenu:` を持つ。
pub(crate) fn registration_from_status(status: i32) -> Result<Registration, DeviceError> {
    match status {
        0 => Ok(Registration::Installed),
        -2 => Err(DeviceError::InvalidInput("app_shortcuts")),
        _ => Err(DeviceError::Internal("app_shortcuts")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_app_shortcuts.m` が定義する C ABI。ポインタと長さは UTF-8 の
    // 文字列 2 本で、`.m` は呼び出しの間に同期でコピーし保持しない。main thread 以外では何もせず -1。
    #[allow(unsafe_code)]
    extern "C" {
        pub fn fandhe_macos_app_shortcuts_install(
            title: *const u8,
            title_len: usize,
            url: *const u8,
            url_len: usize,
        ) -> i32;
    }
}

/// [`AppShortcuts`] の macOS 実装。状態を持たない（一度だけの起動は `InstallCell` が担う）。
#[cfg(target_os = "macos")]
struct MacosAppShortcuts;

#[cfg(target_os = "macos")]
impl AppShortcuts for MacosAppShortcuts {
    fn install(&self) -> Result<Registration, DeviceError> {
        use fandhe_core::screens::url_scheme_show::{SHORTCUT_TITLE, SHORTCUT_URL};
        // SAFETY: 2 本の `&str` は本呼び出しの間生きており、`.m` は同期でコピーするだけ。
        #[allow(unsafe_code)]
        let status = unsafe {
            ffi::fandhe_macos_app_shortcuts_install(
                SHORTCUT_TITLE.as_ptr(),
                SHORTCUT_TITLE.len(),
                SHORTCUT_URL.as_ptr(),
                SHORTCUT_URL.len(),
            )
        };
        registration_from_status(status)
    }
}

/// macOS の実装を返す。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> &'static dyn AppShortcuts {
    &MacosAppShortcuts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping_never_turns_failures_into_success() {
        assert!(matches!(
            registration_from_status(0),
            Ok(Registration::Installed)
        ));
        assert!(matches!(
            registration_from_status(-1),
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(
            registration_from_status(-2),
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(matches!(
            registration_from_status(-3),
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(
            registration_from_status(7),
            Err(DeviceError::Internal(_))
        ));
    }
}
