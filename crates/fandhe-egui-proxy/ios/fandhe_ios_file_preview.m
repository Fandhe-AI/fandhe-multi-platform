// iOS 向けファイル・動画のプレビュー提示（QuickLook `QLPreviewController`、Issue #919、親 #182）。
// `src/device/ios_file_preview.rs` から呼ばれ、`build.rs` が feature `device` 有効時にコンパイルして
// QuickLook をリンクする。Info.plist・entitlement の追加は要らない。サムネイルは
// `apple/fandhe_apple_file_preview.m`（macOS と共用）が担い、本ファイルは提示だけを扱う。
//
// C ABI（Rust 側の `// SAFETY:` の根拠）:
// - `int32_t fandhe_ios_file_preview_present(uint64_t request_id, const char *path)`
//   戻り値 0 = 受理（結果は後で `fandhe_rust_ios_file_preview_outcome` で届く）/ 1 = path 不正。
//   `path` は呼び出しの間だけ有効。return 前に NSString へコピーし、保持しない。
// - Rust 側の受け口: `fandhe_rust_ios_file_preview_outcome(request_id, code)`（1 回だけ。
//   直後に Rust が再描画を要求する）。
//
// 完了コード（`fandhe_device::file_preview::preview_from_macos_code` と一致）:
// 0 = 利用者が閉じた / 1 = 提示できなかった（背景・表示先なし・提示が確定しなかった）。
//
// 状態（delegate の強参照）は main thread だけが触る。パス・NSError・OS の文言はログに出さない。
// 事前判定（canPreviewItem:）はしない（対応形式は OS 依存。表示できなければ OS の画面が示す）。
// 守らない範囲: Rust から閉じること、閉じた通知が届かない場合の復旧、複数 scene、
// 表示中の背景遷移（OS に任せる）、複数プロセス、プロセス死後の結果。

#import <QuickLook/QuickLook.h>
#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"

extern void fandhe_rust_ios_file_preview_outcome(uint64_t request_id, int32_t code);

@interface FandheFilePreviewDelegate : NSObject <QLPreviewControllerDataSource, QLPreviewControllerDelegate>
@property(nonatomic) uint64_t requestId;
@property(nonatomic, copy) NSString *path;
@property(nonatomic) BOOL done;
- (void)finish:(int32_t)code;
@end

// QLPreviewController の dataSource・delegate は weak のため、完了まで強参照で保持する（main thread のみ）。
static FandheFilePreviewDelegate *g_retainedDelegate = nil;

@implementation FandheFilePreviewDelegate
// 完了を 1 回だけ送る。状態（done・強参照）を確定させてから Rust へ渡す。
- (void)finish:(int32_t)code {
    if (self.done) {
        return;
    }
    self.done = YES;
    if (g_retainedDelegate == self) {
        g_retainedDelegate = nil;
    }
    fandhe_rust_ios_file_preview_outcome(self.requestId, code);
}
- (NSInteger)numberOfPreviewItemsInPreviewController:(QLPreviewController *)controller {
    return 1;
}
- (id<QLPreviewItem>)previewController:(QLPreviewController *)controller previewItemAtIndex:(NSInteger)index {
    return [NSURL fileURLWithPath:self.path];
}
- (void)previewControllerDidDismiss:(QLPreviewController *)controller {
    [self finish:0];
}
@end

int32_t fandhe_ios_file_preview_present(uint64_t request_id, const char *path) {
    if (path == NULL) {
        return 1;
    }
    NSString *s = [NSString stringWithUTF8String:path];
    if (s == nil) {
        return 1;
    }
    // egui のフレーム中に UIKit の遷移を始めないため main queue へ積む。
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (UIApplication.sharedApplication.applicationState == UIApplicationStateBackground) {
                fandhe_rust_ios_file_preview_outcome(request_id, 1);
                return;
            }
            UIViewController *top = FandheTopmostViewController();
            if (top == nil || top.view.window == nil || top.presentedViewController != nil ||
                top.isBeingPresented || top.isBeingDismissed) {
                fandhe_rust_ios_file_preview_outcome(request_id, 1);
                return;
            }
            FandheFilePreviewDelegate *delegate = [FandheFilePreviewDelegate new];
            delegate.requestId = request_id;
            delegate.path = s;
            QLPreviewController *vc = [QLPreviewController new];
            vc.dataSource = delegate;
            vc.delegate = delegate;
            // シートのスワイプ閉じなど delegate を通らない経路を作らない。
            vc.modalPresentationStyle = UIModalPresentationFullScreen;
            g_retainedDelegate = delegate;
            // 提示は非同期に確定するため、成否は completion 内で判定する。
            [top presentViewController:vc
                              animated:YES
                            completion:^{
                              if (!delegate.done && top.presentedViewController != vc) {
                                  [delegate finish:1];
                              }
                            }];
        }
    });
    return 0;
}
