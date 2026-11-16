// Phase 11 の拡充ループ（親 #105）型サンプル「セーフエリア」: iOS 実装（Issue #445）。
// key window の `UIView.safeAreaInsets`（iOS 11+）を読む。通知は購読しない（呼び出し側が
// 1 秒間隔で読み直すため、回転・分割表示はその読み直しで追従する）。
//
// `crates/fandhe-egui-proxy/src/device/ios_safe_area_insets.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_ios()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   int32_t fandhe_ios_safe_area_insets_read(double out[4])
//     UI（main）スレッドから同期で呼ぶ。成功時は out に {top, right, bottom, left} を書いて 0。
//     -1: 失敗（main 以外・UIApplication 未確立・key window が無い・out が NULL）。
//     -2 は返さない（配備ターゲットが iOS 17 で、safeAreaInsets は常に使えるため）。
//     値の検査（有限・範囲）と状態コードの写像の SSOT は
//     fandhe_device::safe_area_insets::insets_from_raw。ここでは丸めもクランプもしない。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。main 以外では dispatch_sync せず -1 を返す
// （main 側ロックとのデッドロック回避）。static・購読・ログは持たない（値は機種の推定に
// 使えるためログへ出さない）。

#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"
#include <stdint.h>

int32_t fandhe_ios_safe_area_insets_read(double out[4]) {
    @autoreleasepool {
        if (out == NULL || ![NSThread isMainThread] || UIApplication.sharedApplication == nil) {
            return -1;
        }
        UIWindow *window = FandheRootViewController().view.window;
        if (window == nil) {
            return -1;
        }
        UIEdgeInsets i = window.safeAreaInsets;
        out[0] = i.top;
        out[1] = i.right;
        out[2] = i.bottom;
        out[3] = i.left;
        return 0;
    }
}
