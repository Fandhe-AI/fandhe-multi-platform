//! Windows 向けグローバルホットキーの骨格（Issue #766、親 #150）。実装は #767 が**このファイルだけ**を
//! 埋める。`device/global_hotkey.rs`（effect ドライバ）の windows 経路から呼ばれ、契約は
//! `fandhe_device::global_hotkey`（`GlobalHotkey` / `OnPress` / 結果の 4 種）。
//! ホストテストのため test でもコンパイルする。
//!
//! # #767 への約束
//!
//! - 専用スレッド 1 本で `RegisterHotKey(NULL, id, MOD_CONTROL | MOD_SHIFT | MOD_ALT | MOD_NOREPEAT, 'K')`、
//!   `GetMessageW` で `WM_HOTKEY` の受信、`UnregisterHotKey` を行う（解除は登録したスレッドから）
//! - 登録結果は `fandhe_device::global_hotkey::register_outcome_from_windows` で写す
//!   （`ERROR_HOTKEY_ALREADY_REGISTERED` は使用中）。解除失敗は登録中のまま残し
//!   `Internal("global_hotkey unregister")`
//! - UI スレッドからの呼び出しは専用スレッドの返事を期限なしで同期に待つ（`RegisterHotKey` は即時に戻る）
//! - 押下は同じ [`OnPress`](fandhe_device::global_hotkey::OnPress) へ送る。`shutdown` 経路の解除では
//!   スレッドの終了・join まで行う
//! - dispatch・`device.rs` の `pub mod` 列・共通コア画面（状態・定数・文言）は触らない

use std::sync::Arc;

use fandhe_device::global_hotkey::{GlobalHotkey, UnsupportedGlobalHotkey};

/// Windows の `GlobalHotkey` 実装を返す（骨格の間は非対応）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn implementation() -> Arc<dyn GlobalHotkey> {
    Arc::new(UnsupportedGlobalHotkey)
}
