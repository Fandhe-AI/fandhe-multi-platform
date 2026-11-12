// Phase 11 の拡充ループ（親 #1132）「アクセシビリティ設定の追従」: macOS / iOS 共用の読み取り
// （Issue #1308 が macOS、#1309 が iOS）。
//
// `crates/fandhe-egui-proxy/src/device/apple_accessibility_prefs.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS・iOS 双方の分岐で feature `device` 有効時にコンパイルする。
//
// 契約:
//   int32_t fandhe_apple_accessibility_prefs_read(void)
//     bit0-1 = 視差軽減、bit2-3 = コントラスト増加（各 0 = 不明、1 = オフ、2 = オン）。
//     負値 = 失敗（macOS で `sharedWorkspace` が nil、iOS で main スレッド以外など）。
//
// 購読しない・static を持たない・ログを出さない（値は呼び出しごとに OS から読む）。
// macOS は NSWorkspace を、iOS は UIAccessibility を読む。

#import <TargetConditionals.h>
#import <Foundation/Foundation.h>

#if TARGET_OS_IPHONE

#import <UIKit/UIKit.h>

int32_t fandhe_apple_accessibility_prefs_read(void) {
    // UIAccessibility は UIKit で main スレッド前提。違えば UIKit に触れず失敗で返す
    // （Rust 側で `Internal` になる。「オフ」とは偽らない）。
    if (![NSThread isMainThread]) {
        return -1;
    }
    @autoreleasepool {
        int32_t motion = UIAccessibilityIsReduceMotionEnabled() ? 2 : 1;
        int32_t contrast = UIAccessibilityDarkerSystemColorsEnabled() ? 2 : 1;
        return motion | (contrast << 2);
    }
}

#else

#import <AppKit/AppKit.h>

int32_t fandhe_apple_accessibility_prefs_read(void) {
    @autoreleasepool {
        NSWorkspace *ws = [NSWorkspace sharedWorkspace];
        if (ws == nil) {
            return -1;
        }
        int32_t motion = ws.accessibilityDisplayShouldReduceMotion ? 2 : 1;
        int32_t contrast = ws.accessibilityDisplayShouldIncreaseContrast ? 2 : 1;
        return motion | (contrast << 2);
    }
}

#endif
