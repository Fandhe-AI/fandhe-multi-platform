// Phase 11 の拡充ループ（親 #105）型サンプル「セーフエリア」: macOS 実装（Issue #444）。
// 窓の `contentView.safeAreaInsets`（NSView、macOS 11+）を読む。`NSScreen.safeAreaInsets`
// （画面についての値）は使わない。通知は購読しない（呼び出し側が 1 秒間隔で読み直す）。
//
// `crates/fandhe-egui-proxy/src/device/macos_safe_area_insets.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   int32_t fandhe_macos_safe_area_insets_read(double out[4])
//     UI（main）スレッドから同期で呼ぶ。成功時は out に {top, right, bottom, left} を書いて 0。
//     -1: 失敗（main 以外・NSApp 未確立・窓か contentView が無い・out が NULL）。
//     値の検査（有限・範囲）と状態コードの写像の SSOT は
//     fandhe_device::safe_area_insets::insets_from_raw。ここでは丸めもクランプもしない。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。static・購読・ログは持たない（値は機種の
// 推定に使えるためログへ出さない）。

#import <AppKit/AppKit.h>
#include <stdint.h>

int32_t fandhe_macos_safe_area_insets_read(double out[4]) {
    @autoreleasepool {
        if (out == NULL || ![NSThread isMainThread] || NSApp == nil) {
            return -1;
        }
        NSWindow *window = NSApp.keyWindow ?: NSApp.mainWindow ?: NSApp.windows.firstObject;
        NSView *view = window.contentView;
        if (view == nil) {
            return -1;
        }
        NSEdgeInsets i = view.safeAreaInsets;
        out[0] = i.top;
        out[1] = i.right;
        out[2] = i.bottom;
        out[3] = i.left;
        return 0;
    }
}
