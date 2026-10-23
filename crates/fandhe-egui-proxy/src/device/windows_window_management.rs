//! Windows 向けウィンドウ管理（Issue #636 は骨格のみ、実体は #637、親 #127）。
//! `device/window_management.rs::implementation` の Windows 経路から呼ばれる。現状は
//! `UnsupportedWindowManager` を返す薄いスタブで、対応表の Windows は `possible` のまま。
//!
//! # #637 が埋める範囲
//!
//! - `WindowsReads: NativeReads` の 2 関数だけ。最前面は `GetWindowLongW(GWL_EXSTYLE)` の
//!   `WS_EX_TOPMOST`、作業領域は `MonitorFromWindow` + `GetMonitorInfoW` の `rcWork`
//!   （物理 px を `pixels_per_point` で割って egui point にする）。HWND は `windows_share::window()`
//! - `unsafe` は呼び出し 1 つずつに閉じ、`// SAFETY:` を書く
//! - [`implementation`] を `ViewportWindowManager { ctx, reads: WindowsReads }` を返す形へ差し替える
//! - dispatch・共通コア画面・`registry.rs`・`device.rs` は触らない（#636 で最終形にしてある）

use std::sync::Arc;

use fandhe_device::window_management::{UnsupportedWindowManager, WindowManager};

/// Windows の `WindowManager`。#637 までは非対応固定。
pub(crate) fn implementation(_ctx: egui::Context) -> Arc<dyn WindowManager> {
    Arc::new(UnsupportedWindowManager)
}
