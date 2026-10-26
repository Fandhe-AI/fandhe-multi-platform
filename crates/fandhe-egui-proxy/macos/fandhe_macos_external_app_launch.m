// Phase 11 の拡充ループ（親 #176）型サンプル「電話発信・地図など他アプリの起動」: macOS 実装（Issue #861）。
// `NSWorkspace openURL:` で固定の `tel:` / Apple 地図の URL を開き、既定のアプリに渡す。権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/macos_external_app_launch.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_external_app_open_url(const char *url) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。url は NUL 終端の UTF-8 で、Rust 側の固定値
//     （`fandhe_device::external_app_launch` の電話・地図）だけが渡される。ポインタは呼び出し中だけ
//     読み、保持しない。
//     0 = OS が URL を受理した（アプリが開いたか・発信したかは不明）/ 1 = 扱うアプリが無い（openURL: が NO）
//     2 = NSURL を作れない・main thread ではない
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ログは出さない。

#import <AppKit/AppKit.h>
#include <stdint.h>

int32_t fandhe_macos_external_app_open_url(const char *url) {
    @autoreleasepool {
        if (![NSThread isMainThread] || url == NULL) {
            return 2;
        }
        NSString *s = [NSString stringWithUTF8String:url];
        NSURL *u = s == nil ? nil : [NSURL URLWithString:s];
        if (u == nil) {
            return 2;
        }
        return [[NSWorkspace sharedWorkspace] openURL:u] ? 0 : 1;
    }
}
