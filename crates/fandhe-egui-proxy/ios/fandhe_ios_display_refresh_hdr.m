// Phase 11 の拡充ループ（親 #177）型サンプル「画面のリフレッシュレート・HDR・広色域」:
// iOS 実装（Issue #871）。アプリの窓がある画面 1 枚について、最大 Hz
// （UIScreen.maximumFramesPerSecond）、HDR 用の headroom（UIScreen.potentialEDRHeadroom）、
// 広色域（UITraitCollection.displayGamut）を同期で読む。
// 今の Hz は CADisplayLink で毎フレーム観測しないと取れないため、読まず・推定しない。
// currentEDRHeadroom は EDR を描いている間だけ 1 を超えるので使わない。
//
// `crates/fandhe-egui-proxy/src/device/ios_display_refresh_hdr.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_ios_display_refresh_hdr_read(FandheIosDisplayRaw *out) -> int32_t
//     UI（main）スレッドから同期で呼ぶ。0 = 成功、-1 = 失敗（out が NULL・main 以外・
//     UIApplication 未確立・窓か画面が無い。失敗時は out に書かない）。生値のまま返し、検証の SSOT は
//     Rust 側（fandhe_device::display_refresh_hdr）。ここでは丸めもクランプもしない。
//       max_fps: UIScreen.maximumFramesPerSecond
//       headroom: UIScreen.potentialEDRHeadroom（iOS 16.0+。配備の下限が 17.0 なので @available は不要）
//       p3: displayGamut が P3 なら 1、SRGB なら 0、Unspecified は -1
//
// 窓は root view の window。別の画面へは倒さない（別の画面の値を返さないため）。
// FFI 規約: 全体を `@autoreleasepool` で囲む。main 以外では dispatch_sync せず -1 を返す
// （main 側ロックとのデッドロック回避）。static・購読・ログは持たない。個体識別子は読まない。

#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"
#include <stdint.h>

typedef struct {
    double max_fps;
    double headroom;
    int32_t p3;
} FandheIosDisplayRaw;

int32_t fandhe_ios_display_refresh_hdr_read(FandheIosDisplayRaw *out) {
    @autoreleasepool {
        if (out == NULL || ![NSThread isMainThread] || UIApplication.sharedApplication == nil) {
            return -1;
        }
        UIWindow *window = FandheRootViewController().view.window;
        UIScreen *screen = window.windowScene.screen;
        if (window == nil || screen == nil) {
            return -1;
        }
        int32_t p3 = -1;
        switch (screen.traitCollection.displayGamut) {
            case UIDisplayGamutP3:
                p3 = 1;
                break;
            case UIDisplayGamutSRGB:
                p3 = 0;
                break;
            default:
                break;
        }
        out->max_fps = (double)screen.maximumFramesPerSecond;
        out->headroom = (double)screen.potentialEDRHeadroom;
        out->p3 = p3;
        return 0;
    }
}
