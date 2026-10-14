// Phase 11 の拡充ループ（親 #1145）型サンプル「OS 搭載フォント」: macOS 実装（Issue #1376）。
// AppKit の `NSFontManager.sharedFontManager.availableFontFamilies` で、端末にあるフォントの
// ファミリー名を同期で読む。書体（fullName・PostScript 名・スタイル）・ファイルパス・バイト列は
// 読まない。名前はログに出さない（フィンガープリントになりうるため）。権限の宣言は増やさない。
//
// `crates/fandhe-egui-proxy/src/device/macos_local_fonts.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_local_fonts_list(buf, cap, out_len, out_count, out_overflowed) -> int32_t
//     UI スレッドから同期で呼ぶ。0 = 成功、非 0 = 失敗（out_* は無効）。
//     buf（呼び出し側が確保した cap バイト）へ、名前を UTF-8 の NUL 終端で連続して詰める。
//     1 名前は 4095 バイトまで（UTF-8 の文字境界で切る）。128 文字などの整形上限は Rust 側 clean_text が持ち、
//     ここで切るのは異常に長い名前への保険のみ。out_len は書いた総バイト数、
//     out_count は OS が返したファミリー数（バッファに入った数ではない）。
//     名前が残りの領域（4096 バイト未満）に収まらなくなったらそこで止め、*out_overflowed = 1 にする。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#include <stddef.h>
#include <stdint.h>

#define FANDHE_FONT_NAME_MAX 4095

int32_t fandhe_macos_local_fonts_list(uint8_t *buf, size_t cap, size_t *out_len,
                                      uint32_t *out_count, int32_t *out_overflowed) {
    if (buf == NULL || out_len == NULL || out_count == NULL || out_overflowed == NULL) {
        return 1;
    }
    @autoreleasepool {
        NSArray<NSString *> *families = [NSFontManager sharedFontManager].availableFontFamilies;
        if (families == nil) {
            return 2;
        }
        size_t pos = 0;
        *out_overflowed = 0;
        *out_count = (uint32_t)families.count;
        for (NSString *name in families) {
            if (cap - pos < FANDHE_FONT_NAME_MAX + 1) {
                *out_overflowed = 1;
                break;
            }
            NSUInteger used = 0;
            // maxLength 内に収まる文字までを書く（文字の途中では切らない）。
            [name getBytes:buf + pos
                 maxLength:FANDHE_FONT_NAME_MAX
                usedLength:&used
                  encoding:NSUTF8StringEncoding
                   options:0
                     range:NSMakeRange(0, name.length)
            remainingRange:NULL];
            pos += used;
            buf[pos++] = 0;
        }
        *out_len = pos;
    }
    return 0;
}
