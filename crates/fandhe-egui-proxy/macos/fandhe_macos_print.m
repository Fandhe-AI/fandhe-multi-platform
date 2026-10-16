// Phase 11 の拡充ループ（親 #94）型サンプル「印刷」: macOS 実装（Issue #325）。
// `NSPrintPanel` で取り消しを先に確定し、`showsPrintPanel = NO` の `NSPrintOperation` で実行する
// 2 段構成。1 回の `NSPrintOperation` だと `runOperation` の NO が取り消しか失敗か区別できない。
// 権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/macos_print.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_print_text(const char *title, const char *body) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。title / body は NUL 終端の UTF-8 で、Rust 側で
//     検査済み（制御文字なし・body は '\n' 区切り）。ポインタは呼び出し中だけ読み、保持しない。
//     0 = Submitted（パネル OK かつ runOperation が YES）
//     1 = Cancelled（パネルが NSModalResponseCancel）
//     2 = 提示不可（main thread 外・NULL・NSString 化失敗）
//     3 = 実行失敗（パネル OK 後の runOperation が NO）
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ログは出さない（見出し・本文を含むため）。
// 文字列はプレーンな NSAttributedString として扱い、HTML / RTF として解釈しない。

#import <AppKit/AppKit.h>
#include <stdint.h>

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

        NSPrintPanel *panel = [NSPrintPanel printPanel];
        if ([panel runModalWithPrintInfo:info] != NSModalResponseOK) {
            return 1;
        }

        // 画面外の NSTextView に見出し + 本文を入れ、1 ページ分のレイアウトを確定させる。
        CGFloat width = info.paperSize.width - info.leftMargin - info.rightMargin;
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

        NSPrintOperation *op = [NSPrintOperation printOperationWithView:tv printInfo:info];
        op.showsPrintPanel = NO;
        op.showsProgressPanel = YES;
        op.jobTitle = titleStr;
        return [op runOperation] ? 0 : 3;
    }
}
