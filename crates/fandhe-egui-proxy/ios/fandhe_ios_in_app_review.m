// Phase 11 の拡充ループ型サンプル「アプリ内レビュー依頼」: iOS 実装（Issue #564）。
// 前面の UIWindowScene を渡して `[SKStoreReviewController requestReviewInScene:]` を 1 回呼ぶ。権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/ios_in_app_review.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルし、StoreKit をリンクする。
//
// 契約:
//   fandhe_ios_in_app_review_request(void) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。
//     0 = 依頼を渡した / 2 = main thread ではない
//     3 = 前面（ForegroundActive）の application role の UIWindowScene が無い
//     keyWindow・windows へは逃がさない（背景にいる間に依頼を出さないため）。
//     表示・投稿されたかは返らない（この API に completion は無い）。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは受け取らず保持しない。ログは出さない。

#import <StoreKit/StoreKit.h>
#import <UIKit/UIKit.h>
#include <stdint.h>

int32_t fandhe_ios_in_app_review_request(void) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
        for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
            if (![scene isKindOfClass:[UIWindowScene class]]
                || scene.activationState != UISceneActivationStateForegroundActive
                || ![scene.session.role isEqualToString:UIWindowSceneSessionRoleApplication]) {
                continue;
            }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            [SKStoreReviewController requestReviewInScene:(UIWindowScene *)scene];
#pragma clang diagnostic pop
            return 0;
        }
        return 3;
    }
}
