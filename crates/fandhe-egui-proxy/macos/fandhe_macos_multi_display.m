// Phase 11 の拡充ループ（親 #1134）「複数ディスプレイ」: macOS 実装（Issue #1341）。
// `[NSScreen screens]` を毎回読み、画面ごとの物理ピクセル（`frame` × `backingScaleFactor`）・
// 倍率・主画面か（配列の先頭）・名前（`localizedName`）を返す。個体識別子
// （`CGDisplaySerialNumber` / EDID）は読まない。名前・値をログへ出さない。
//
// `crates/fandhe-egui-proxy/src/device/macos_multi_display.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_multi_display_read(FandheScreenRaw *out, uint32_t cap, uint32_t *count) -> int32_t
//     main thread から同期で呼ぶ。main thread 以外では `dispatch_sync` せず失敗を返す
//     （main 側のロックとのデッドロックを避ける）。0 = 成功、非 0 = 失敗（out / count は無効）。
//     失敗: 引数が NULL・cap が 0・main thread 以外・画面数が cap 超過（先頭だけを返して
//     個数を偽らない）・サイズや倍率が不正（非有限・0 以下・UINT32_MAX 超）。
//     is_primary: 配列の先頭だけ 1（`mainScreen` はキーウィンドウ依存のため使わない）。
//     name: NUL 終端の UTF-8。取れない・128 バイトに収まらない場合は空文字（切り詰めない）。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <AppKit/AppKit.h>
#include <math.h>
#include <stdint.h>
#include <string.h>

typedef struct {
    uint32_t width_px;
    uint32_t height_px;
    double scale;
    uint8_t is_primary;
    char name[128];
} FandheScreenRaw;

int32_t fandhe_macos_multi_display_read(FandheScreenRaw *out, uint32_t cap, uint32_t *count) {
    if (out == NULL || count == NULL || cap == 0) {
        return 1;
    }
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        NSArray<NSScreen *> *screens = [NSScreen screens];
        NSUInteger n = screens.count;
        if (n > cap) {
            return 3;
        }
        for (NSUInteger i = 0; i < n; i++) {
            NSScreen *s = screens[i];
            double scale = s.backingScaleFactor;
            NSSize size = s.frame.size;
            if (!isfinite(scale) || scale <= 0.0 || !isfinite(size.width) ||
                !isfinite(size.height)) {
                return 4;
            }
            double w = round(size.width * scale);
            double h = round(size.height * scale);
            if (w <= 0.0 || h <= 0.0 || w > (double)UINT32_MAX || h > (double)UINT32_MAX) {
                return 4;
            }
            FandheScreenRaw *r = &out[i];
            memset(r, 0, sizeof(*r));
            r->width_px = (uint32_t)w;
            r->height_px = (uint32_t)h;
            r->scale = scale;
            r->is_primary = (i == 0) ? 1 : 0;
            if ([s respondsToSelector:@selector(localizedName)]) {
                const char *utf8 = s.localizedName.UTF8String;
                if (utf8 != NULL && strlen(utf8) < sizeof(r->name)) {
                    strcpy(r->name, utf8);
                }
            }
        }
        *count = (uint32_t)n;
    }
    return 0;
}
