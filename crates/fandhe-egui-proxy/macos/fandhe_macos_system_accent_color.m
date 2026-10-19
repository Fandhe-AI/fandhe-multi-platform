// Phase 11 の拡充ループ（親 #103）型サンプル「システムのアクセント色」: macOS 実装（Issue #421）。
// `NSColor.controlAccentColor` を `NSApp.effectiveAppearance` の下で sRGB に解決して読む。
// 通知（NSSystemColorsDidChangeNotification）は購読しない（呼び出し側が 1 秒間隔で読み直す）。
//
// `crates/fandhe-egui-proxy/src/device/macos_system_accent_color.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   int32_t fandhe_macos_system_accent_color_read(void)
//     UI（main）スレッドから同期で呼ぶ。
//     0..=0xFFFFFF: 0xRRGGBB。-1: 失敗（main 以外・NSApp 未確立・変換失敗・成分が不正）。
//     値の写像の SSOT は fandhe_device::system_accent_color::rgb_from_code。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。static・購読・ログは持たない（色は
// フィンガープリントに使えるためログへ出さない）。

#import <AppKit/AppKit.h>
#include <math.h>
#include <stdint.h>

static int32_t channel(CGFloat c) {
    if (isnan(c)) {
        return -1;
    }
    if (c < 0.0) c = 0.0;
    if (c > 1.0) c = 1.0;
    return (int32_t)lround(c * 255.0);
}

int32_t fandhe_macos_system_accent_color_read(void) {
    @autoreleasepool {
        if (![NSThread isMainThread] || NSApp == nil) {
            return -1;
        }
        __block int32_t code = -1;
        [NSApp.effectiveAppearance performAsCurrentDrawingAppearance:^{
            NSColor *c = [[NSColor controlAccentColor]
                colorUsingColorSpace:NSColorSpace.sRGBColorSpace];
            if (c == nil) {
                return;
            }
            int32_t r = channel(c.redComponent);
            int32_t g = channel(c.greenComponent);
            int32_t b = channel(c.blueComponent);
            if (r < 0 || g < 0 || b < 0) {
                return;
            }
            code = (r << 16) | (g << 8) | b;
        }];
        return code;
    }
}
