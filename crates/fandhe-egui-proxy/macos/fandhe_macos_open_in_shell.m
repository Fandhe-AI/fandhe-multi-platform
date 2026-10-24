// Phase 11 の拡充ループ（親 #146）型サンプル「既定アプリで開く・Finder で表示」: macOS 実装
// （Issue #749）。`NSWorkspace` で既定アプリを開く / Finder で選択表示する。権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/macos_open_in_shell.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_open_in_shell_open(const char *path) -> int32_t
//   fandhe_macos_open_in_shell_reveal(const char *path) -> int32_t
//     path は NUL 終端の UTF-8 で、Rust 側（`ShellPath`）で検査・正規化済みの絶対パスだけが渡される。
//     ポインタは呼び出し中だけ読み、保持しない。どのスレッドから呼んでもよい（公式）が、実際には
//     UI スレッドから同期で呼ばれる。
//     0 = OS が受理した（アプリ・ウィンドウが開いたかは不明）
//     1 = 扱うアプリが無い（open のみ。URLForApplicationToOpenURL: が nil）
//     2 = 拒否した型（open のみ。実行可能・アプリ・パッケージ、または判定できない）
//     3 = OS の失敗・文字列変換の失敗
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ログは出さない（パスを含むため）。

#import <AppKit/AppKit.h>
#include <stdint.h>

static NSURL *fandhe_url_from_path(const char *path) {
    if (path == NULL) {
        return nil;
    }
    NSString *s = [NSString stringWithUTF8String:path];
    if (s == nil) {
        return nil;
    }
    // 文字列を URL として解釈させず、ファイルパスとして 1 つの引数で渡す。
    return [NSURL fileURLWithPath:s isDirectory:NO];
}

// 実行可能・アプリ・パッケージは true。読めないときも true（fail-closed）。
static bool fandhe_is_dangerous(NSURL *url) {
    NSArray<NSURLResourceKey> *keys = @[
        NSURLIsExecutableKey, NSURLIsApplicationKey, NSURLIsPackageKey
    ];
    for (NSURLResourceKey key in keys) {
        NSNumber *value = nil;
        NSError *error = nil;
        if (![url getResourceValue:&value forKey:key error:&error] || value == nil ||
            [value boolValue]) {
            return true;
        }
    }
    return false;
}

int32_t fandhe_macos_open_in_shell_open(const char *path) {
    @autoreleasepool {
        NSURL *url = fandhe_url_from_path(path);
        if (url == nil) {
            return 3;
        }
        if (fandhe_is_dangerous(url)) {
            return 2;
        }
        NSWorkspace *ws = [NSWorkspace sharedWorkspace];
        if ([ws URLForApplicationToOpenURL:url] == nil) {
            return 1;
        }
        return [ws openURL:url] ? 0 : 3;
    }
}

int32_t fandhe_macos_open_in_shell_reveal(const char *path) {
    @autoreleasepool {
        NSString *s = path == NULL ? nil : [NSString stringWithUTF8String:path];
        if (s == nil) {
            return 3;
        }
        return [[NSWorkspace sharedWorkspace] selectFile:s inFileViewerRootedAtPath:@""] ? 0 : 3;
    }
}
