// Phase 11 の拡充ループ型サンプル「アプリ内レビュー依頼」: macOS 実装（Issue #563）。
// `[SKStoreReviewController requestReview]` を 1 回呼ぶ。権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/macos_in_app_review.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし、StoreKit をリンクする。
//
// 契約:
//   fandhe_macos_in_app_review_request(void) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。
//     0 = 依頼を渡した / 2 = main thread ではない
//     表示・投稿されたかは返らない。未バンドル実行（cargo run）で表示されるかは #556 の確認項目。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは受け取らず保持しない。ログは出さない。

#import <StoreKit/StoreKit.h>
#include <stdint.h>

int32_t fandhe_macos_in_app_review_request(void) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        [SKStoreReviewController requestReview];
#pragma clang diagnostic pop
        return 0;
    }
}
