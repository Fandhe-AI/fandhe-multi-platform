// iOS 向け印刷（Issue #327）。`src/device/ios_print.rs` から呼ばれ、`UIPrintInteractionController` の
// 印刷シートを main queue で提示する。権限・Info.plist キー・追加 framework は不要。
//
// C ABI:
//   int32_t fandhe_ios_print_present(uint64_t request_id, const char *title, const char *body)
//     戻り値: 0 = 受理（結果は後で非同期に届く）、1 = 受理できない（NULL・不正な UTF-8）。
//     文字列は return 前に NSString へコピーし、所有権は移らない。
//   extern void fandhe_rust_print_outcome(uint64_t request_id, int32_t code)
//     code は `device/print.rs::outcome_from_ios_code` の表と一致させる
//     （0 Submitted / 1 Cancelled / 2 error あり / 3 提示不可 / 4 印刷不可）。
//     各依頼につきちょうど 1 回だけ呼ぶ。
//
// 見出し・本文は HTML として解釈しない（`UIMarkupTextPrintFormatter` は使わない）。
// 見出し・本文・NSError はログへ出さない。

#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"

extern void fandhe_rust_print_outcome(uint64_t request_id, int32_t code);

// completion と提示失敗の両経路で結果を 1 回だけ届けるための holder。
@interface FandhePrintOnce : NSObject
@property(nonatomic) uint64_t requestId;
@property(nonatomic) BOOL done;
@end

@implementation FandhePrintOnce
// 先に done を立ててから届ける（届けた後に片付ける順序にしない）。
- (void)finish:(int32_t)code {
    if (self.done) {
        return;
    }
    self.done = YES;
    fandhe_rust_print_outcome(self.requestId, code);
}
@end

int32_t fandhe_ios_print_present(uint64_t request_id, const char *title, const char *body) {
    if (title == NULL || body == NULL) {
        return 1;
    }
    NSString *titleS = [NSString stringWithUTF8String:title];
    NSString *bodyS = [NSString stringWithUTF8String:body];
    if (titleS == nil || bodyS == nil) {
        return 1;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            FandhePrintOnce *once = [FandhePrintOnce new];
            once.requestId = request_id;
            if (![UIPrintInteractionController isPrintingAvailable]) {
                [once finish:4];
                return;
            }
            UIPrintInteractionController *pc = [UIPrintInteractionController sharedPrintController];
            UIPrintInfo *info = [UIPrintInfo printInfo];
            info.outputType = UIPrintInfoOutputGeneral;
            info.jobName = titleS;
            pc.printInfo = info;

            NSMutableAttributedString *text = [NSMutableAttributedString new];
            [text appendAttributedString:[[NSAttributedString alloc]
                initWithString:[titleS stringByAppendingString:@"\n\n"]
                    attributes:@{NSFontAttributeName : [UIFont boldSystemFontOfSize:18]}]];
            [text appendAttributedString:[[NSAttributedString alloc]
                initWithString:bodyS
                    attributes:@{NSFontAttributeName : [UIFont systemFontOfSize:12]}]];
            UISimpleTextPrintFormatter *fmt =
                [[UISimpleTextPrintFormatter alloc] initWithAttributedText:text];
            fmt.perPageContentInsets = UIEdgeInsetsMake(72, 72, 72, 72);
            pc.printFormatter = fmt;

            UIPrintInteractionCompletionHandler handler =
                ^(UIPrintInteractionController *_Nonnull __unused c, BOOL completed, NSError *error) {
                  if (error != nil) {
                      [once finish:2];
                  } else {
                      [once finish:completed ? 0 : 1];
                  }
                };
            BOOL shown;
            if (UIDevice.currentDevice.userInterfaceIdiom == UIUserInterfaceIdiomPad) {
                // iPad は popover 提示。基準点は top view の中央。
                UIViewController *top = FandheTopmostViewController();
                if (top == nil || top.view == nil) {
                    [once finish:3];
                    return;
                }
                CGRect b = top.view.bounds;
                CGRect r = CGRectMake(CGRectGetMidX(b), CGRectGetMidY(b), 1, 1);
                shown = [pc presentFromRect:r inView:top.view animated:YES completionHandler:handler];
            } else {
                shown = [pc presentAnimated:YES completionHandler:handler];
            }
            if (!shown) {
                [once finish:3];
            }
        }
    });
    return 0;
}
