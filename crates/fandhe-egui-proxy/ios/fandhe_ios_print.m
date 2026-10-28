// iOS 向け印刷（Issue #327）。`src/device/ios_print.rs` から呼ばれ、`UIPrintInteractionController` の
// 印刷シートを main queue で提示する。権限・Info.plist キー・追加 framework は不要。
//
// C ABI:
//   int32_t fandhe_ios_print_present(uint64_t request_id, const char *title, const char *body)
//     戻り値: 0 = 受理（結果は後で非同期に届く）、1 = 受理できない（NULL・不正な UTF-8）。
//     文字列は return 前に NSString へコピーし、所有権は移らない。
//   int32_t fandhe_ios_print_present_image(uint64_t request_id, const uint8_t *rgba, size_t len,
//                                          uint32_t width, uint32_t height)
//     画面の画像（print-screen・Issue #1011）。rgba は RGBA 8bit（premultiplied）・行を詰めた
//     width*height*4 バイトで、Rust 側（PrintImage）で検査済み。.m でも len を再検査する。
//     戻り値: 0 = 受理（結果は後で非同期に届く）、1 = 受理できない（NULL・0 寸法・長さ不一致）。
//     画素は return 前に NSData へコピーし、所有権は移らない。結果コード表は文章版と共用。
//   extern void fandhe_rust_print_outcome(uint64_t request_id, int32_t code)
//     code は `device/print.rs::outcome_from_ios_code` の表と一致させる
//     （0 Submitted / 1 Cancelled / 2 error あり / 3 提示不可 / 4 印刷不可）。
//     各依頼につきちょうど 1 回だけ呼ぶ。
//
// 見出し・本文は HTML として解釈しない（`UIMarkupTextPrintFormatter` は使わない）。
// 見出し・本文・画素・NSError はログへ出さない。

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

// 提示の共通部（文章版・画像版で共用）。`configure` で印刷対象を設定する。main queue 上で呼ぶこと。
static void FandhePrintPresent(uint64_t request_id, NSString *jobName,
                               void (^configure)(UIPrintInteractionController *pc)) {
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
            info.jobName = jobName;
            pc.printInfo = info;
            // 共有コントローラに前回の対象が残る。これらは排他のため先に全部外す。
            pc.printingItem = nil;
            pc.printingItems = nil;
            pc.printFormatter = nil;
            pc.printPageRenderer = nil;
            configure(pc);

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
}

int32_t fandhe_ios_print_present(uint64_t request_id, const char *title, const char *body) {
    if (title == NULL || body == NULL) {
        return 1;
    }
    NSString *titleS = [NSString stringWithUTF8String:title];
    NSString *bodyS = [NSString stringWithUTF8String:body];
    if (titleS == nil || bodyS == nil) {
        return 1;
    }
    FandhePrintPresent(request_id, titleS, ^(UIPrintInteractionController *pc) {
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
    });
    return 0;
}

int32_t fandhe_ios_print_present_image(uint64_t request_id, const uint8_t *rgba, size_t len,
                                       uint32_t width, uint32_t height) {
    if (rgba == NULL || width == 0 || height == 0) {
        return 1;
    }
    // 長さの再検査（乗算の桁あふれも弾く）。
    size_t bytesPerRow = (size_t)width * 4;
    if (bytesPerRow / 4 != (size_t)width || bytesPerRow > SIZE_MAX / (size_t)height ||
        len != bytesPerRow * (size_t)height) {
        return 1;
    }
    // Rust 側の PrintImage は return 後に drop されるため、非同期ブロックへ渡す前にコピーする。
    NSData *data = [NSData dataWithBytes:rgba length:len];
    if (data == nil) {
        return 1;
    }
    FandhePrintPresent(request_id, @"fandhe screen", ^(UIPrintInteractionController *pc) {
        CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();
        CGDataProviderRef dp = CGDataProviderCreateWithCFData((__bridge CFDataRef)data);
        CGImageRef cg = NULL;
        if (cs != NULL && dp != NULL) {
            // egui の Color32 は premultiplied。行は上から下で反転不要。
            cg = CGImageCreate(width, height, 8, 32, bytesPerRow, cs,
                               (CGBitmapInfo)kCGImageAlphaPremultipliedLast | kCGBitmapByteOrderDefault,
                               dp, NULL, false, kCGRenderingIntentDefault);
        }
        if (cs != NULL) {
            CGColorSpaceRelease(cs);
        }
        if (dp != NULL) {
            CGDataProviderRelease(dp);
        }
        if (cg != NULL) {
            // 生成失敗時は printingItem が nil のまま提示され、UIKit が提示不可（3）で返す。
            pc.printingItem = [UIImage imageWithCGImage:cg];
            CGImageRelease(cg);
        }
    });
    return 0;
}
