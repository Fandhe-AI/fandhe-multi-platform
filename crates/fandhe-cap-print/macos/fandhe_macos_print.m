// Phase 11 の拡充ループ（親 #94）型サンプル「印刷」: macOS 実装（Issue #325）。
// 画面の内容の印刷（print-screen、親 #341、Issue #1010）も同じファイルで、ダイアログの出し方・
// 結果コードは文章と共用する。
// `NSPrintPanel` で取り消しを先に確定し、`showsPrintPanel = NO` の `NSPrintOperation` で実行する
// 2 段構成。1 回の `NSPrintOperation` だと `runOperation` の NO が取り消しか失敗か区別できない。
// 権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/macos_print.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_print_text(const char *title, const char *body) -> int32_t
//   fandhe_macos_print_image(const uint8_t *rgba, size_t len, uint32_t width, uint32_t height) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。title / body は NUL 終端の UTF-8 で、Rust 側で
//     検査済み（制御文字なし・body は '\n' 区切り）。rgba は RGBA 8bit・行を詰めた width*height*4 バイトで、
//     Rust 側（PrintImage）で検査済み。.m でも len を再検査する。ポインタは呼び出し中だけ読み、保持しない。
//     0 = Submitted（パネル OK かつ runOperation が YES）
//     1 = Cancelled（パネルが NSModalResponseCancel）
//     2 = 提示不可（main thread 外・NULL・NSString / NSBitmapImageRep 化失敗・長さ不一致）
//     3 = 実行失敗（パネル OK 後の runOperation が NO）
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ログは出さない（見出し・本文・画素を含むため）。
// 文字列はプレーンな NSAttributedString として扱い、HTML / RTF として解釈しない。

#import <AppKit/AppKit.h>
#include <stdint.h>
#include <string.h>

// パネル → パネルなしの実行の 2 段構成。`makeView` はパネル OK のあとで（用紙・余白が確定してから）
// 印刷する view を作る。呼び出し側の `@autoreleasepool` の内側で呼ぶこと。
static int32_t fandhe_print_two_step(NSPrintInfo *info, NSString *jobTitle,
                                     NSView * (^makeView)(NSPrintInfo *info)) {
    NSPrintPanel *panel = [NSPrintPanel printPanel];
    if ([panel runModalWithPrintInfo:info] != NSModalResponseOK) {
        return 1;
    }
    NSView *view = makeView(info);
    if (view == nil) {
        return 3;
    }
    NSPrintOperation *op = [NSPrintOperation printOperationWithView:view printInfo:info];
    op.showsPrintPanel = NO;
    op.showsProgressPanel = YES;
    op.jobTitle = jobTitle;
    return [op runOperation] ? 0 : 3;
}

int32_t fandhe_macos_print_text(const char *title, const char *body) {
    @autoreleasepool {
        if (![NSThread isMainThread] || title == NULL || body == NULL) {
            return 2;
        }
        NSString *titleStr = [NSString stringWithUTF8String:title];
        NSString *bodyStr = [NSString stringWithUTF8String:body];
        if (titleStr == nil || bodyStr == nil) {
            return 2;
        }

        // 共有設定を汚さないよう複製して使う。
        NSPrintInfo *info = [[NSPrintInfo sharedPrintInfo] copy];
        // 0 は自動改ページ。定数名は SDK 世代で異なる（NSAutoPagination は macOS 11 で deprecated）ため使わない。
        info.horizontalPagination = (NSPrintingPaginationMode)0;
        info.verticalPagination = (NSPrintingPaginationMode)0;
        info.verticallyCentered = NO;

        return fandhe_print_two_step(info, titleStr, ^NSView *(NSPrintInfo *pi) {
            // 画面外の NSTextView に見出し + 本文を入れ、1 ページ分のレイアウトを確定させる。
            CGFloat width = pi.paperSize.width - pi.leftMargin - pi.rightMargin;
            NSTextView *tv = [[NSTextView alloc] initWithFrame:NSMakeRect(0, 0, width, 100)];
            tv.verticallyResizable = YES;
            tv.horizontallyResizable = NO;
            NSMutableAttributedString *text = [[NSMutableAttributedString alloc] init];
            [text appendAttributedString:
                [[NSAttributedString alloc]
                    initWithString:[titleStr stringByAppendingString:@"\n"]
                        attributes:@{NSFontAttributeName: [NSFont boldSystemFontOfSize:18]}]];
            [text appendAttributedString:
                [[NSAttributedString alloc]
                    initWithString:bodyStr
                        attributes:@{NSFontAttributeName: [NSFont systemFontOfSize:12]}]];
            [tv.textStorage setAttributedString:text];
            [tv sizeToFit];
            return tv;
        });
    }
}

int32_t fandhe_macos_print_image(const uint8_t *rgba, size_t len, uint32_t width, uint32_t height) {
    @autoreleasepool {
        if (![NSThread isMainThread] || rgba == NULL || width == 0 || height == 0) {
            return 2;
        }
        // 長さの再検査（乗算の桁あふれも弾く）。
        size_t bytesPerRow = (size_t)width * 4;
        if (bytesPerRow / 4 != (size_t)width || bytesPerRow > SIZE_MAX / (size_t)height ||
            len != bytesPerRow * (size_t)height) {
            return 2;
        }

        NSBitmapImageRep *rep = [[NSBitmapImageRep alloc]
            initWithBitmapDataPlanes:NULL
                          pixelsWide:width
                          pixelsHigh:height
                       bitsPerSample:8
                     samplesPerPixel:4
                            hasAlpha:YES
                            isPlanar:NO
                      colorSpaceName:NSDeviceRGBColorSpace
                         bytesPerRow:(NSInteger)bytesPerRow
                        bitsPerPixel:32];
        // 行を詰めた並びで確保できたときだけ一括コピーする（そうでなければ書き込み範囲を保証できない）。
        if (rep == nil || rep.bitmapData == NULL || (size_t)rep.bytesPerRow != bytesPerRow) {
            return 2;
        }
        memcpy(rep.bitmapData, rgba, len);
        NSImage *image = [[NSImage alloc] initWithSize:NSMakeSize(width, height)];
        [image addRepresentation:rep];

        NSPrintInfo *info = [[NSPrintInfo sharedPrintInfo] copy];
        // 1 は Fit（1 ページに収める）。定数名は SDK 世代で異なるため使わない（文章版と同じ理由）。
        info.horizontalPagination = (NSPrintingPaginationMode)1;
        info.verticalPagination = (NSPrintingPaginationMode)1;
        info.horizontallyCentered = YES;
        info.verticallyCentered = YES;

        // ジョブ名は固定。画素は出さない。
        return fandhe_print_two_step(info, @"fandhe screen", ^NSView *(NSPrintInfo *pi) {
            // 印刷可能領域（用紙 - 余白）いっぱいの NSImageView に、縦横比を保って収める。
            NSSize area = NSMakeSize(pi.paperSize.width - pi.leftMargin - pi.rightMargin,
                                     pi.paperSize.height - pi.topMargin - pi.bottomMargin);
            if (area.width < 1 || area.height < 1) {
                return nil;
            }
            NSImageView *iv = [[NSImageView alloc]
                initWithFrame:NSMakeRect(0, 0, area.width, area.height)];
            iv.image = image;
            iv.imageScaling = NSImageScaleProportionallyUpOrDown;
            iv.imageAlignment = NSImageAlignCenter;
            return iv;
        });
    }
}
