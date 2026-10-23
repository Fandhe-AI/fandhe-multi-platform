//! macOS 向けウィンドウ管理の読み取り（Issue #636、親 #127）。
//! `macos/fandhe_macos_window_management.m`（窓の `level` と、窓がいる画面の `visibleFrame`。
//! `build.rs` が macOS ターゲットの feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` は呼び出し 1 つずつに閉じる。
//! 変更（位置・大きさ・最前面）は `device/window_management.rs` の `ViewportWindowManager` が
//! eframe の `ViewportCommand` で行い、ここは [`NativeReads`] の 2 関数だけを担う。
//! 契約は `fandhe_device::window_management`。座標変換と値の検査の SSOT は同モジュールの
//! `frame_from_cocoa`。座標・画面の大きさはログへ出さない。
//!
//! # 既知の天井
//!
//! - 複数窓では main window（なければ key window）の値だけを見る。無ければ読めない（`None`）
//! - 実機の目視確認（正常系・失敗系・複数画面）は #629

use std::sync::Arc;

use fandhe_device::window_management::{frame_from_cocoa, WindowFrame, WindowManager};

use super::window_management::{NativeReads, ViewportWindowManager};

// SAFETY: `macos/fandhe_macos_window_management.m` が定義する C ABI。
// `fandhe_macos_window_level_is_floating` は書き込み可能な `i32` 1 つへのポインタ、
// `fandhe_macos_window_visible_frame` は書き込み可能な 5 要素の `double` 配列の先頭ポインタを取り、
// どちらも戻り値 0 のときだけ main スレッド上で同期に書く。main スレッド以外・NSApp 未確立・
// 窓や画面なしでは何も書かず -1 を返す。
#[allow(unsafe_code)]
extern "C" {
    fn fandhe_macos_window_level_is_floating(out: *mut i32) -> i32;
    fn fandhe_macos_window_visible_frame(out: *mut f64) -> i32;
}

/// [`NativeReads`] の macOS 実装。状態を持たない。
pub(crate) struct MacosReads;

impl NativeReads for MacosReads {
    fn always_on_top(&self) -> Option<bool> {
        let mut out: i32 = 0;
        // SAFETY: `out` は呼び出しの間生きている書き込み可能な `i32` で、上記 extern 宣言の契約に合う。
        #[allow(unsafe_code)]
        let status = unsafe { fandhe_macos_window_level_is_floating(&mut out) };
        (status == 0).then_some(out != 0)
    }

    fn work_area(&self, ctx: &egui::Context) -> Option<WindowFrame> {
        let mut raw = [0f64; 5];
        // SAFETY: `raw` は呼び出しの間生きている 5 要素の書き込み可能バッファで、上記 extern
        // 宣言の契約（成功時に 5 要素を書くのみ）に合う。
        #[allow(unsafe_code)]
        let status = unsafe { fandhe_macos_window_visible_frame(raw.as_mut_ptr()) };
        if status != 0 {
            return None;
        }
        frame_from_cocoa(
            [raw[0], raw[1], raw[2], raw[3]],
            raw[4],
            f64::from(ctx.zoom_factor()),
        )
    }
}

/// macOS の `WindowManager` を返す。`device/window_management.rs::implementation` から呼ばれる。
pub(crate) fn implementation(ctx: egui::Context) -> Arc<dyn WindowManager> {
    Arc::new(ViewportWindowManager {
        ctx,
        reads: MacosReads,
    })
}
