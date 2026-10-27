// Phase 11 の拡充ループ（親 #188）型サンプル「壁紙・ロック画面の設定」: macOS 実装（Issue #970）。
// `NSWorkspace.setDesktopImageURL:forScreen:options:error:` を `NSScreen.screens` の全画面へ呼び、
// `desktopImageURLForScreen:` で読み戻して設定した URL と一致したときだけ成功とする。権限は不要
// （sandbox なしのアプリ）。ロック画面の壁紙は公開 API が無く、Rust 側で `Unsupported` にする。
//
// `crates/fandhe-egui-proxy/src/device/macos_wallpaper.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_wallpaper_set(const char *path_utf8) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。path は呼び出しの間だけ有効な借用で、保持しない。
//     0 = 全画面で設定し、読み戻しも一致
//     2 = main thread ではない（AppKit に触らず返す。dispatch_sync は使わない）
//     3 = setDesktopImageURL が NSError を返した
//     4 = 読み戻しが nil または不一致（一部の画面だけ成功した場合も含む）
//     5 = 画面が 0 枚
//     6 = path が NULL または不正
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ログは出さない（パス・NSError を出さない）。

#import <AppKit/AppKit.h>
#include <stdint.h>

static NSString *fandhe_wallpaper_canonical(NSURL *url) {
    return [[url URLByStandardizingPath] URLByResolvingSymlinksInPath].path;
}

int32_t fandhe_macos_wallpaper_set(const char *path_utf8) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
        if (path_utf8 == NULL) {
            return 6;
        }
        NSString *path = [NSString stringWithUTF8String:path_utf8];
        if (path == nil || path.length == 0) {
            return 6;
        }
        NSURL *url = [NSURL fileURLWithPath:path];
        NSArray<NSScreen *> *screens = NSScreen.screens;
        if (screens.count == 0) {
            return 5;
        }
        NSWorkspace *workspace = NSWorkspace.sharedWorkspace;
        for (NSScreen *screen in screens) {
            NSError *error = nil;
            if (![workspace setDesktopImageURL:url forScreen:screen options:@{} error:&error]) {
                return 3;
            }
        }
        NSString *expected = fandhe_wallpaper_canonical(url);
        for (NSScreen *screen in screens) {
            NSURL *current = [workspace desktopImageURLForScreen:screen];
            if (current == nil || ![fandhe_wallpaper_canonical(current) isEqualToString:expected]) {
                return 4;
            }
        }
        return 0;
    }
}
