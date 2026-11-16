//! macOS 向けアプリショートカット実装（Issue #424、親 #101）。Dock メニューに「デモを開く」を出す。
//! `macos/fandhe_macos_app_shortcuts.m`（`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じる。
//! `driver::platform_app_shortcuts` の macOS 経路から main thread で呼ばれ、契約は
//! `crate::contract`。メニュー項目が選ばれると `.m` が `fandhe-cap-url-scheme` の
//! `fandhe_rust_url_scheme_received`（`apple.rs`）へ固定 URL を渡し、`UrlInbox` で検査される
//! （機能 crate 同士の依存。リンクから落ちないよう下で `register` を参照する）。
//!
//! # 既知の天井
//!
//! - Dock メニューは起動中だけ出る。他アプリが同じ URL を送っても区別しない
//! - winit 0.30.13 の `WinitApplicationDelegate` は `applicationDockMenu:` を実装していない
//!   前提で、そのクラスへメソッドを足す。既にある場合は上書きせず失敗として返す
//! - 実機の目視（AppKit が Dock 右クリックのたびに selector を引くか）は #413

#[cfg(target_os = "macos")]
use crate::contract::{AppShortcuts, ShortcutSpec};

// `.m` が呼ぶ `fandhe_rust_url_scheme_received` は url-scheme の `apple.rs` にある。同じ codegen unit の公開関数を
// 実際に参照して、リンク対象から外れないようにする（`capability-crate-migration.md` §21）。
#[cfg(target_os = "macos")]
const _: fn() = fandhe_cap_url_scheme::register;
use crate::contract::Registration;
use crate::error::AppShortcutsError;

/// `.m` の戻り値を契約上の結果へ写像する（SSOT）。
/// 0: 登録した / -1: main 以外か NSApp・delegate が無い / -2: 文字列を作れない /
/// -3: delegate が既に `applicationDockMenu:` を持つ。
pub(crate) fn registration_from_status(status: i32) -> Result<Registration, AppShortcutsError> {
    match status {
        0 => Ok(Registration::Installed),
        -2 => Err(AppShortcutsError::InvalidInput("app_shortcuts")),
        _ => Err(AppShortcutsError::Internal("app_shortcuts")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_app_shortcuts.m` が定義する C ABI。ポインタと長さは UTF-8 の
    // 文字列 2 本で、`.m` は呼び出しの間に同期でコピーし保持しない。main thread 以外では何もせず -1。
    #[allow(unsafe_code)]
    unsafe extern "C" {
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
    fn install(&self, spec: &ShortcutSpec<'_>) -> Result<Registration, AppShortcutsError> {
        // SAFETY: `spec` の 2 本の `&str` は本呼び出しの間生きており、`.m` は同期でコピーするだけ。
        #[allow(unsafe_code)]
        let status = unsafe {
            ffi::fandhe_macos_app_shortcuts_install(
                spec.title.as_ptr(),
                spec.title.len(),
                spec.url.as_ptr(),
                spec.url.len(),
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
            Err(AppShortcutsError::Internal(_))
        ));
        assert!(matches!(
            registration_from_status(-2),
            Err(AppShortcutsError::InvalidInput(_))
        ));
        assert!(matches!(
            registration_from_status(-3),
            Err(AppShortcutsError::Internal(_))
        ));
        assert!(matches!(
            registration_from_status(7),
            Err(AppShortcutsError::Internal(_))
        ));
    }
}
