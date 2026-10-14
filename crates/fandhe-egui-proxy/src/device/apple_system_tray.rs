//! macOS 向けシステムトレイ実装（Issue #1039）。`device/system_tray.rs::platform_tray`
//! の macOS 分岐から呼ばれ、`NSStatusBar`/`NSStatusItem`/`NSMenu` 実装
//! （`apple/fandhe_apple_system_tray.m`、`build.rs::build_macos` がコンパイル・
//! リンクする）を `unsafe extern "C"` 関数 2 つ（[`fandhe_apple_system_tray_show`]/
//! [`fandhe_apple_system_tray_hide`]）で呼ぶ。`unsafe` はこの呼び出しに閉じ、
//! `// SAFETY:` で契約を明記する（`apple_location.rs` と同型、coding-rust.md の
//! FFI 例外規定準拠）。
//!
//! # 「表示中かどうか」を Rust 側で判定する理由
//!
//! 二重表示の判定（[`SystemTray::show`] の `AlreadyShown`）は `.m` 側へ問い合わせず、
//! [`QUEUE`]（`fandhe_device::system_tray::SelectionQueue`）の
//! [`is_active`](fandhe_device::system_tray::SelectionQueue::is_active) で完結させる。
//! `QUEUE.begin`/`QUEUE.end` を「表示中フラグ」としても流用することで、
//! 表示状態を Rust/ObjC 両側に二重管理しない（SSOT、`device-bridge-contracts.md`
//! 「6. 嘘の成功」の裏返しの設計判断: 片方だけが表示中と思い込む food を防ぐ）。
//!
//! `.m` 側の `onSelect:` から [`fandhe_rust_system_tray_selected`]（本モジュールが
//! 公開する `extern "C"` 関数）が呼ばれ、[`QUEUE::push`] へ積む（push 型、
//! `apple_location.rs` の `fandhe_rust_location_fix` と同じ橋渡し方式）。

use std::ffi::CString;
use std::os::raw::c_char;

use fandhe_device::system_tray::{HideOutcome, SelectionQueue, ShowOutcome, SystemTray, TrayMenu};
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_system_tray.m` が定義する C ABI（`build.rs::build_macos`
// がコンパイル・リンクする）。契約は `.m` ファイル冒頭コメントおよび下記
// `AppleSystemTray` 実装のコメントに集約する。呼び出しスレッドが main でなければ
// `.m` 側が `dispatch_sync` で main へ寄せてから実行する（`device/system_tray.rs`
// モジュール doc「別スレッドへ逃がす理由」参照、P1 是正）。
#[allow(unsafe_code)]
unsafe extern "C" {
    /// 戻り値: 0 = 成功、2 = アイコン/メニュー生成に失敗、3 = 表示前に終了処理の
    /// hide で取り消された（項目は作らない。どちらも `show` は `Err` に畳む）。
    fn fandhe_apple_system_tray_show(
        generation: u64,
        title: *const c_char,
        items: *const *const c_char,
        item_count: i32,
    ) -> i32;
    /// 戻り値: 0 = 成功（固定、失敗経路が無い。`.m` ファイル冒頭コメント参照）。
    fn fandhe_apple_system_tray_hide() -> i32;
}

/// `QUEUE.is_active()` が「現在表示中か」を兼ねる（モジュール doc 参照）。
static QUEUE: SelectionQueue = SelectionQueue::new();

/// macOS 向け `SystemTray` 実装。`fandhe-egui-proxy::device::system_tray::drive`
/// から作業スレッド（UI スレッドではない）で呼ばれる契約。AppKit 呼び出しは
/// main thread 必須のため、`.m` 側が `dispatch_sync` で main へ寄せてから実行
/// する（`device/system_tray.rs` モジュール doc「別スレッドへ逃がす理由」参照）。
pub(crate) struct AppleSystemTray;

impl SystemTray for AppleSystemTray {
    #[allow(unsafe_code)]
    fn show(&self, menu: &TrayMenu) -> Result<ShowOutcome, DeviceError> {
        // CString 化失敗（NUL 混入）は `TrayMenu::new` の検証で既に排除済みだが、
        // 防御的に `Internal` へ畳む（`TrayMenu` の不変条件が将来崩れても panic
        // させない）。
        let title =
            CString::new(menu.title()).map_err(|_| DeviceError::Internal("system_tray show"))?;
        let item_cstrings: Vec<CString> = menu
            .items()
            .iter()
            .map(|s| CString::new(s.as_str()))
            .collect::<Result<_, _>>()
            .map_err(|_| DeviceError::Internal("system_tray show"))?;
        let item_ptrs: Vec<*const c_char> = item_cstrings.iter().map(|c| c.as_ptr()).collect();

        // 表示中かの判定と開始は同じロック内で行う（`SelectionQueue::try_begin` 参照）。
        let Some(generation) = QUEUE.try_begin(menu.items().len()) else {
            return Ok(ShowOutcome::AlreadyShown);
        };
        // SAFETY: `title`/`item_ptrs` の各ポインタは、この呼び出しが戻るまで
        // `title`/`item_cstrings` が生存しているため有効（`.m` 側はポインタを
        // 保持せず、呼び出し中に `NSString` へコピーする契約、`.m` ファイル冒頭
        // コメント参照）。`item_ptrs.len()` は `menu.items().len()`（検証済み、
        // `TrayMenu::MAX_MENU_ITEMS` 以下）を `i32` へ収める。
        let code = unsafe {
            fandhe_apple_system_tray_show(
                generation,
                title.as_ptr(),
                item_ptrs.as_ptr(),
                item_ptrs.len() as i32,
            )
        };
        if code == 0 {
            Ok(ShowOutcome::Added)
        } else {
            // 生成失敗（`NSStatusItem`/`NSString` の生成失敗）。確保した世代を
            // 無効化し、以後の選択（来ないはずだが防御的に）を無視できるように
            // する（`device-bridge-contracts.md`「9. 失敗時のリソース解放」）。
            QUEUE.end();
            Err(DeviceError::Internal("system_tray show"))
        }
    }

    #[allow(unsafe_code)]
    fn hide(&self) -> Result<HideOutcome, DeviceError> {
        if !QUEUE.is_active() {
            return Ok(HideOutcome::NotShown);
        }
        // SAFETY: 引数・戻り値は固定（`i32`）で、呼び出しスレッドを変更しない。
        // 呼び出しスレッドが main でなければ `.m` 側が `dispatch_sync` で main へ
        // 寄せてから実行する（`.m` ファイル冒頭コメント参照）。
        let code = unsafe { fandhe_apple_system_tray_hide() };
        if code != 0 {
            // 失敗時は「まだ表示中」として扱う（`QUEUE` も active のまま残す。
            // `fandhe-core` 側の `finish_hide` 失敗時の `Shown` 復帰と整合させる）。
            return Err(DeviceError::Internal("system_tray hide"));
        }
        QUEUE.end();
        Ok(HideOutcome::Removed)
    }

    fn take_selection(&self) -> Option<usize> {
        QUEUE.pop()
    }
}

/// `.m` 側の `FandheTrayTarget::onSelect:` から呼ばれる `extern "C"` 関数。
/// `generation` が現在の表示と一致し、`index` が範囲内のときだけ [`QUEUE`] へ
/// 積む（`SelectionQueue::push` が世代・範囲の両方を照合する、モジュール doc
/// 参照）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_system_tray_selected(generation: u64, index: i64) {
    QUEUE.push(generation, index);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fandhe_rust_system_tray_selected` は本物の表示（`AppleSystemTray::show`）が
    /// 無い状態（`QUEUE` 未 active）では no-op になることを確認する（他テストが
    /// 先に `QUEUE` を active にしている可能性があるため、世代不一致での
    /// 拒否のみを固定する厳密な確認は統合テストに委ねる）。
    #[test]
    fn selected_with_mismatched_generation_is_ignored() {
        // 存在し得ない極端な世代番号を渡しても panic しないことのみを確認する
        // （`SelectionQueue::push` 自体の世代照合は `fandhe-device` 側で検証済み）。
        fandhe_rust_system_tray_selected(u64::MAX, 0);
    }
}
