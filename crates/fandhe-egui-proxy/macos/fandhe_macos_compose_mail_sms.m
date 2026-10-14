// Phase 11 の拡充ループ（親 #1142）型サンプル「メール・SMS の作成」: macOS 実装（Issue #1384）。
// `NSWorkspace openURL:` で `mailto:` / `sms:` の URL を開き、既定のメール / メッセージアプリの
// 作成画面を出す。権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/macos_compose_mail_sms.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_compose_open_url(const char *url) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。url は NUL 終端の UTF-8 で、Rust 側で検査・
//     percent-encode 済みの `mailto:` / `sms:` URL だけが渡される。ポインタは呼び出し中だけ読み、
//     保持しない。
//     0 = OS が URL を受理した（開いたか・送ったかは不明）/ 1 = 扱うアプリが無い（openURL: が NO）
//     2 = NSURL を作れない・main thread ではない
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ログは出さない（宛先・件名・本文を含むため）。

#import <AppKit/AppKit.h>
#include <stdint.h>

int32_t fandhe_macos_compose_open_url(const char *url) {
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
