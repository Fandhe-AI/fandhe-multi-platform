// Phase 11 の拡充ループ（親 #127）型サンプル「ウィンドウ管理」: macOS 実装（Issue #636）。
// 位置・大きさ・最前面の「変更」は eframe の ViewportCommand（winit）が行うため、ここは読み取り専用。
// egui の viewport 情報に無い 2 つだけを読む: 窓の level（最前面か）と、窓がいる画面の作業領域
// （メニューバー・Dock を除く visibleFrame）。
//
// `crates/fandhe-egui-proxy/src/device/macos_window_management.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約（どちらも UI（main）スレッドから同期で呼ぶ。-1: 失敗 = main 以外・NSApp 未確立・
// 窓か画面が無い・out が NULL。失敗時は out に書かない）:
//   int32_t fandhe_macos_window_level_is_floating(int32_t *out)
//     成功時は out に 1（level が NSNormalWindowLevel より上 = winit の AlwaysOnTop）か 0 を書いて 0。
//   int32_t fandhe_macos_window_visible_frame(double out[5])
//     成功時は out に {x, y, 幅, 高さ, 主画面の高さ} を Cocoa 座標（左下原点）のまま書いて 0。
//     y 軸の反転・zoom での除算・値の検査の SSOT は
//     fandhe_device::window_management::frame_from_cocoa。ここでは丸めもクランプもしない。
//
// 窓は NSApp.mainWindow ?: NSApp.keyWindow。windows.firstObject へは倒さない（別の窓を読んで
// 偽の状態を返さないため）。FFI 規約: 全体を `@autoreleasepool` で囲み、main 以外では
// dispatch_sync せず失敗を返す。static・通知の購読・ログは持たない（画面配置は利用環境の推定に使えるため）。

#import <AppKit/AppKit.h>
#include <stdint.h>

static NSWindow *fandhe_target_window(void) {
    return NSApp.mainWindow ?: NSApp.keyWindow;
}

int32_t fandhe_macos_window_level_is_floating(int32_t *out) {
    @autoreleasepool {
        if (out == NULL || ![NSThread isMainThread] || NSApp == nil) {
            return -1;
        }
        NSWindow *window = fandhe_target_window();
        if (window == nil) {
            return -1;
        }
        *out = window.level > NSNormalWindowLevel ? 1 : 0;
        return 0;
    }
}

int32_t fandhe_macos_window_visible_frame(double out[5]) {
    @autoreleasepool {
        if (out == NULL || ![NSThread isMainThread] || NSApp == nil) {
            return -1;
        }
        NSWindow *window = fandhe_target_window();
        NSScreen *screen = window.screen;
        NSScreen *primary = NSScreen.screens.firstObject;
        if (window == nil || screen == nil || primary == nil) {
            return -1;
        }
        NSRect v = screen.visibleFrame;
        out[0] = v.origin.x;
        out[1] = v.origin.y;
        out[2] = v.size.width;
        out[3] = v.size.height;
        out[4] = primary.frame.size.height;
        return 0;
    }
}
