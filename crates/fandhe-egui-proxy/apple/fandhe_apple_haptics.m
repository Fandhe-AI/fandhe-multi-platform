// Phase 11 の拡充ループ（親 #1090）型サンプル「触覚フィードバック」: macOS 実装（Issue #1091）
// と iOS 実装（Issue #1092）。
//
// `crates/fandhe-egui-proxy/src/device/apple_haptics.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS（AppKit）・iOS（UIKit + CoreHaptics）ターゲットの feature `device`
// 有効時に `cc` コンパイルする。Rust 側 cfg（`device.rs` の `apple_haptics`）と同じ OS 集合。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_apple_haptics_tap(void) -> int32_t
//     0 = 要求を渡した（`NSHapticFeedbackManager` の `perform` は戻り値が無く、
//         Force Touch トラックパッド以外では何も起きず、それを検知できない）。
//     1 = performer を取得できなかった（呼び出し元が `Internal` へ畳む）。
//     2 = main スレッド以外から呼ばれた（AppKit / UIKit に触れずに返す。呼び出し元が `Internal` へ畳む）。
//     3 = 触覚非搭載（iOS のみ。`CHHapticEngine` の `supportsHaptics` が NO。シミュレータ・
//         iPad の多くが該当。呼び出し元が `Unsupported` へ写す。macOS は 3 を返さない）。
// iOS の `0` も「要求を渡した」まで（サイレント・システム触覚オフ・背景では無反応で検知不可）。
//
// スレッド: AppKit は main スレッド前提。呼び出し元（`device/haptics.rs` の effect）は
// egui の update ループ = macOS では main スレッドから同期で呼ぶ。performer の取得・判定・
// 実行は main 上の同じ処理で行い、失敗をそのまま返す。main 以外から呼ばれた場合は
// `dispatch_sync` で main を待たず（main 側が呼び出し元を待っているとデッドロックするため、
// `device-bridge-contracts.md`「11」）、AppKit に触れずに 2 を返す。
// Cocoa / UIKit 呼び出しは `@autoreleasepool` の内側で行う。iOS も egui の update = main
// から同期で呼ばれ、同じく `dispatch_sync` は使わない。
//
// 振動以外の情報（デバイス名等）は扱わない・ログへ出さない（security.md A09）。

#import <TargetConditionals.h>
#if TARGET_OS_OSX
#import <AppKit/AppKit.h>

int32_t fandhe_apple_haptics_tap(void) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        id<NSHapticFeedbackPerformer> performer =
            [NSHapticFeedbackManager defaultPerformer];
        if (performer == nil) {
            return 1;
        }
        [performer performFeedbackPattern:NSHapticFeedbackPatternGeneric
                          performanceTime:NSHapticFeedbackPerformanceTimeNow];
    }
    return 0;
}
#elif TARGET_OS_IPHONE
#import <CoreHaptics/CoreHaptics.h>
#import <UIKit/UIKit.h>

int32_t fandhe_apple_haptics_tap(void) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        // 非搭載機では `impactOccurred` が無反応なため、呼ぶ前に判定して成功扱いにしない。
        if (![CHHapticEngine capabilitiesForHardware].supportsHaptics) {
            return 3;
        }
        UIImpactFeedbackGenerator *generator =
            [[UIImpactFeedbackGenerator alloc] initWithStyle:UIImpactFeedbackStyleLight];
        [generator impactOccurred];
    }
    return 0;
}
#endif
