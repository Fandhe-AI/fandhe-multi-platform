// iOS 向けアプリ内ブラウザ（SafariServices `SFSafariViewController`、Issue #702、親 #135）。
// `src/device/ios_in_app_browser.rs` から呼ばれ、`build.rs` が feature `device` 有効時に
// コンパイルして SafariServices をリンクする。Info.plist・entitlement の追加は要らない。
//
// C ABI（Rust 側の `// SAFETY:` の根拠）:
// - `int32_t fandhe_ios_in_app_browser_open(uint64_t request_id, const char *url)`
//   戻り値 0 = 受理（結果は後で `fandhe_rust_in_app_browser_complete` で届く）/
//   1 = URL 不正（NULL・不正な UTF-8・NSURL にならない・http/https 以外・host なし）/ 2 = その他。
//   `url` は呼び出しの間だけ有効。return 前に NSURL へコピーし、保持しない。
// - Rust 側の受け口: `fandhe_rust_in_app_browser_try_begin_present(request_id)`（1 = 表示してよい）、
//   `fandhe_rust_in_app_browser_complete(request_id, code)`（1 回だけ。直後に Rust が再描画を要求する）。
//
// 完了コード: 0 = 利用者が閉じた / 1 = 表示先なし / 2 = 表示前に背景遷移 / 3 = URL 不正 /
// 4 = 提示されなかった。Rust 側の `outcome_from_ios_code` と一致させる。
//
// 状態（delegate の強参照）は main thread だけが触る。URL・NSError・OS の文言はログに出さない。
// 守らない範囲: 開いた後に Rust から閉じること、閲覧内容・遷移先・読み込み失敗の検知、
// 表示中の背景遷移（OS に任せる）、複数プロセス、プロセス死後の結果。

#import <SafariServices/SafariServices.h>
#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"

extern int32_t fandhe_rust_in_app_browser_try_begin_present(uint64_t request_id);
extern void fandhe_rust_in_app_browser_complete(uint64_t request_id, int32_t code);

@interface FandheInAppBrowserDelegate : NSObject <SFSafariViewControllerDelegate, UIAdaptivePresentationControllerDelegate>
@property(nonatomic) uint64_t requestId;
@property(nonatomic) BOOL done;
- (void)finish:(int32_t)code;
@end

// SFSafariViewController.delegate は weak のため、完了まで強参照で保持する（main thread のみ）。
static FandheInAppBrowserDelegate *g_retainedDelegate = nil;

@implementation FandheInAppBrowserDelegate
// 完了を 1 回だけ送る。状態（done・強参照）を確定させてから Rust へ渡す。
- (void)finish:(int32_t)code {
    if (self.done) {
        return;
    }
    self.done = YES;
    if (g_retainedDelegate == self) {
        g_retainedDelegate = nil;
    }
    fandhe_rust_in_app_browser_complete(self.requestId, code);
}
- (void)safariViewControllerDidFinish:(SFSafariViewController *)controller {
    [self finish:0];
}
- (void)presentationControllerDidDismiss:(UIPresentationController *)presentationController {
    [self finish:0];
}
@end

static BOOL FandheIsHttpUrl(NSURL *url) {
    NSString *scheme = url.scheme.lowercaseString;
    return ([scheme isEqualToString:@"http"] || [scheme isEqualToString:@"https"]) && url.host.length > 0;
}

int32_t fandhe_ios_in_app_browser_open(uint64_t request_id, const char *url) {
    if (url == NULL) {
        return 1;
    }
    NSString *s = [NSString stringWithUTF8String:url];
    NSURL *target = s ? [NSURL URLWithString:s] : nil;
    if (target == nil || !FandheIsHttpUrl(target)) {
        return 1;
    }
    // egui のフレーム中に UIKit の遷移を始めないため main queue へ積む。
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (UIApplication.sharedApplication.applicationState == UIApplicationStateBackground) {
                fandhe_rust_in_app_browser_complete(request_id, 2);
                return;
            }
            // SFSafariViewController は http/https 以外で例外を投げるため、init より前で再確認する。
            if (!FandheIsHttpUrl(target)) {
                fandhe_rust_in_app_browser_complete(request_id, 3);
                return;
            }
            UIViewController *top = FandheTopmostViewController();
            if (top == nil || top.view.window == nil || top.presentedViewController != nil ||
                top.isBeingPresented || top.isBeingDismissed) {
                fandhe_rust_in_app_browser_complete(request_id, 1);
                return;
            }
            // 期限切れ・古い要求なら Rust が 0 を返す。その場合は表示も完了送信もしない。
            if (fandhe_rust_in_app_browser_try_begin_present(request_id) == 0) {
                return;
            }
            FandheInAppBrowserDelegate *delegate = [FandheInAppBrowserDelegate new];
            delegate.requestId = request_id;
            SFSafariViewController *vc = [[SFSafariViewController alloc] initWithURL:target];
            vc.delegate = delegate;
            // delegate を通らない閉じ方（スワイプ）を塞ぐ。
            vc.modalInPresentation = YES;
            vc.presentationController.delegate = delegate;
            g_retainedDelegate = delegate;
            // 提示は非同期に確定するため、成否は completion 内で判定する。
            [top presentViewController:vc
                              animated:YES
                            completion:^{
                              if (!delegate.done && top.presentedViewController != vc) {
                                  [delegate finish:4];
                              }
                            }];
        }
    });
    return 0;
}
