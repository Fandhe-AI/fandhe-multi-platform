// Phase 11 の拡充ループ（親 #177）型サンプル「画面のリフレッシュレート・HDR・広色域」:
// macOS 実装（Issue #870）。アプリの窓がある画面 1 枚について、最大 Hz
// （NSScreen.maximumFramesPerSecond、12.0+）、今の Hz（CGDisplayModeGetRefreshRate）、HDR 用の
// headroom（maximumPotentialExtendedDynamicRangeColorComponentValue）、広色域
// （canRepresentDisplayGamut:NSDisplayGamutP3）を同期で読む。
// maximumExtendedDynamicRangeColorComponentValue は EDR を描いている間だけ 1 を超えるので使わない。
//
// `crates/fandhe-egui-proxy/src/device/macos_display_refresh_hdr.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_display_refresh_hdr_read(FandheDisplayRaw *out) -> int32_t
//     UI（main）スレッドから同期で呼ぶ。0 = 成功、-1 = 失敗（out が NULL・main 以外・NSApp 未確立・
//     窓か画面が無い。失敗時は out に書かない）。生値のまま返し、検証の SSOT は Rust 側
//     （fandhe_device::display_refresh_hdr）。ここでは丸めもクランプもしない。
//       max_fps / current_hz: 取れなければ -1
//       headroom: maximumPotentialExtendedDynamicRangeColorComponentValue（10.15+ で常に取れる）
//       p3: 1 / 0
//
// 窓は NSApp.mainWindow ?: NSApp.keyWindow。windows.firstObject へは倒さない（別の窓の画面を読んで
// 偽の値を返さないため）。個体識別子（NSScreenNumber）は CGDirectDisplayID への変換にだけ使い、
// 返さず・ログに出さない。FFI 規約: 全体を `@autoreleasepool` で囲む。static・購読・ログは持たない。

#import <AppKit/AppKit.h>
#include <CoreGraphics/CoreGraphics.h>
#include <stdint.h>

typedef struct {
    double max_fps;
    double current_hz;
    double headroom;
    int32_t p3;
} FandheDisplayRaw;

int32_t fandhe_macos_display_refresh_hdr_read(FandheDisplayRaw *out) {
    @autoreleasepool {
        if (out == NULL || ![NSThread isMainThread] || NSApp == nil) {
            return -1;
        }
        NSWindow *window = NSApp.mainWindow ?: NSApp.keyWindow;
        NSScreen *screen = window.screen;
        if (window == nil || screen == nil) {
            return -1;
        }

        double max_fps = -1;
        if (@available(macOS 12.0, *)) {
            max_fps = (double)screen.maximumFramesPerSecond;
        }

        double current_hz = -1;
        NSNumber *number = screen.deviceDescription[@"NSScreenNumber"];
        if (number != nil) {
            CGDisplayModeRef mode = CGDisplayCopyDisplayMode((CGDirectDisplayID)number.unsignedIntValue);
            if (mode != NULL) {
                current_hz = CGDisplayModeGetRefreshRate(mode);
                CGDisplayModeRelease(mode);
            }
        }

        out->max_fps = max_fps;
        out->current_hz = current_hz;
        out->headroom = (double)screen.maximumPotentialExtendedDynamicRangeColorComponentValue;
        out->p3 = [screen canRepresentDisplayGamut:NSDisplayGamutP3] ? 1 : 0;
        return 0;
    }
}
