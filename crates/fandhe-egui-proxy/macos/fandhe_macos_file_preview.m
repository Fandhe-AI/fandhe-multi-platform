// Phase 11 の拡充ループ（親 #182）型サンプル「ファイル・動画のプレビューとサムネイル」:
// macOS（Issue #917）のプレビュー実装。自前の窓に `QLPreviewView` を載せて固定ファイルを表示し、
// 窓が閉じた通知で Rust へ「閉じた」を伝える。
//
// `crates/fandhe-egui-proxy/src/device/macos_file_preview.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし、QuickLookUI をリンクする。
//
// 契約（pull 型。`fandhe_macos_eye_dropper.m` と同型）:
//   int32_t fandhe_macos_file_preview_present(uint64_t request_id, const char *path)
//     path を NSString にコピーしてから main queue へ dispatch し、すぐ 0 を返す。
//     結果は非同期に下の関数で届く。
//   fandhe_rust_file_preview_outcome(uint64_t request_id, int32_t code)
//     -> Rust 側（`macos_file_preview.rs`）が公開する `extern "C"` 関数。
//        code 表: 0 = 窓が閉じた、1 = 提示できなかった（NSApp が無い・URL や view を作れない）。
//        code → 結果の写像の SSOT は Rust の `fandhe_device::file_preview::preview_from_macos_code`。
//
// スレッド: 窓・view の生成と通知ハンドラはすべて main queue。完了通知は 1 要求につき 1 回
// （`delivered` で守る。Rust 側も RequestSlot で first-wins）。窓とオブザーバーはハンドラが
// 強参照で保持し、通知後に nil にして解放する。QLPreviewView は shouldCloseWithWindow が既定の YES
// なので、窓が閉じるときの close は OS に任せる。
//
// OS を正にする: `canPreviewItem:` などの事前判定はしない。閉じたことは窓の通知だけで知る。
// 閉じる公開 API と期限は持たない。通知が届かなかった場合の復旧は守らない範囲。
//
// 情報非保持: パスをログへ出さない（`NSLog` 不使用、security.md A09）。
// ObjC の例外を Rust へ通さないため、例外を投げる API は使わない。

#import <AppKit/AppKit.h>
#import <QuickLookUI/QuickLookUI.h>
#include <stdint.h>

extern void fandhe_rust_file_preview_outcome(uint64_t request_id, int32_t code);

int32_t fandhe_macos_file_preview_present(uint64_t request_id, const char *path) {
    @autoreleasepool {
        NSString *p = path == NULL ? nil : [NSString stringWithUTF8String:path];
        if (p == nil) {
            return 1;
        }
        dispatch_async(dispatch_get_main_queue(), ^{
            @autoreleasepool {
                NSURL *url = [NSURL fileURLWithPath:p];
                if (NSApp == nil || url == nil) {
                    fandhe_rust_file_preview_outcome(request_id, 1);
                    return;
                }
                NSRect frame = NSMakeRect(0, 0, 640, 480);
                NSWindowStyleMask style = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                          NSWindowStyleMaskResizable | NSWindowStyleMaskMiniaturizable;
                __block NSWindow *window = [[NSWindow alloc] initWithContentRect:frame
                                                                       styleMask:style
                                                                         backing:NSBackingStoreBuffered
                                                                           defer:NO];
                QLPreviewView *view = [[QLPreviewView alloc] initWithFrame:frame
                                                                     style:QLPreviewViewStyleNormal];
                if (window == nil || view == nil) {
                    fandhe_rust_file_preview_outcome(request_id, 1);
                    return;
                }
                window.releasedWhenClosed = NO;
                [window setTitleWithRepresentedFilename:p];
                view.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
                view.previewItem = url;
                window.contentView = view;

                __block BOOL delivered = NO;
                __block id token = nil;
                token = [[NSNotificationCenter defaultCenter]
                    addObserverForName:NSWindowWillCloseNotification
                                object:window
                                 queue:[NSOperationQueue mainQueue]
                            usingBlock:^(NSNotification *_Nonnull __unused note) {
                                if (!delivered) {
                                    delivered = YES;
                                    fandhe_rust_file_preview_outcome(request_id, 0);
                                }
                                if (token != nil) {
                                    [[NSNotificationCenter defaultCenter] removeObserver:token];
                                    token = nil;
                                }
                                window = nil;
                            }];
                [window center];
                [window makeKeyAndOrderFront:nil];
            }
        });
        return 0;
    }
}
