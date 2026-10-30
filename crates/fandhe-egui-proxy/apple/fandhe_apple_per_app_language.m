// Phase 11 の拡充ループ（親 #137）「アプリ別の言語設定」: macOS / iOS 共用の読み取り
// （Issue #1073 が macOS、#1074 が iOS）。
//
// `crates/fandhe-egui-proxy/src/device/apple_per_app_language.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS・iOS 双方の分岐で feature `device` 有効時にコンパイルする。
//
// 契約:
//   int32_t fandhe_apple_per_app_language_read(uint8_t *out, int32_t cap)
//     `[[NSBundle mainBundle] preferredLocalizations].firstObject` を `out` に NUL 終端の UTF-8 で書く。
//     戻り値: 0 = 成功、-1 = nil・空・`UTF8String` が NULL、-2 = `cap - 1` バイトに収まらない（`out` は書かない）。
//
// 購読しない・static を持たない・ログを出さない（値は呼び出しごとに OS から読む）。

#import <Foundation/Foundation.h>

int32_t fandhe_apple_per_app_language_read(uint8_t *out, int32_t cap) {
    if (out == NULL || cap < 2) {
        return -2;
    }
    @autoreleasepool {
        NSString *first = [[NSBundle mainBundle] preferredLocalizations].firstObject;
        if (first == nil || first.length == 0) {
            return -1;
        }
        const char *utf8 = first.UTF8String;
        if (utf8 == NULL) {
            return -1;
        }
        size_t len = strlen(utf8);
        if (len > (size_t)(cap - 1)) {
            return -2;
        }
        memcpy(out, utf8, len);
        out[len] = '\0';
        return 0;
    }
}
