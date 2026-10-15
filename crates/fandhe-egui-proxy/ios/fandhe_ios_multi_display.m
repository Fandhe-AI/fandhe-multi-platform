// Phase 11 の拡充ループ（親 #1134）「複数ディスプレイ」: iOS 実装（Issue #27）。
// `UIApplication.connectedScenes` の `UIWindowScene.screen` を毎回読み、画面ごとの物理ピクセル
// （`bounds.size` × `scale`）・倍率・主画面か（application ロールの scene の画面）を返す。
// 外部ディスプレイは `UIWindowSceneSessionRoleExternalDisplayNonInteractive` の scene として
// 接続される（`UIScreen.screens` は iOS 16 で非推奨。scene 化は `fandhe_ios_scene.m`）。
// 画面名は `UIScreen` に表示名の API が無いため常に空（Rust 側で `None`）。個体識別子は読まず、
// 値をログへ出さない。ミラーリング中の扱いは OS が返す scene のとおり。
//
// `crates/fandhe-egui-proxy/src/device/ios_multi_display.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_ios_multi_display_read(FandheScreenRaw *out, uint32_t cap, uint32_t *count) -> int32_t
//     main thread から同期で呼ぶ。main thread 以外では `dispatch_sync` せず失敗を返す
//     （main 側のロックとのデッドロックを避ける）。0 = 成功、非 0 = 失敗（out / count は無効）。
//     失敗: 引数が NULL・cap が 0・main thread 以外・接続中の window scene が 0 件（空の一覧を
//     成功にしない）・画面数が cap 超過（先頭だけを返して個数を偽らない）・サイズや倍率が不正
//     （非有限・0 以下・UINT32_MAX 超）。
//     並び順: 主画面が先頭、残りは（幅, 高さ）の昇順。`connectedScenes` は順序が不定の Set のため、
//     読むたびに並びが入れ替わって「変化」と誤認されないよう固定する。
//     is_primary: application ロールの scene の画面だけ 1。見つからなければ全て 0。
//     name: 常に空文字。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <UIKit/UIKit.h>
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

static int fandhe_screen_cmp(const void *a, const void *b) {
    const FandheScreenRaw *x = a, *y = b;
    if (x->is_primary != y->is_primary) {
        return x->is_primary ? -1 : 1;
    }
    if (x->width_px != y->width_px) {
        return x->width_px < y->width_px ? -1 : 1;
    }
    if (x->height_px != y->height_px) {
        return x->height_px < y->height_px ? -1 : 1;
    }
    return 0;
}

int32_t fandhe_ios_multi_display_read(FandheScreenRaw *out, uint32_t cap, uint32_t *count) {
    if (out == NULL || count == NULL || cap == 0) {
        return 1;
    }
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        NSMutableArray<UIScreen *> *seen = [NSMutableArray array];
        uint32_t n = 0;
        for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
            if (![scene isKindOfClass:[UIWindowScene class]]) {
                continue;
            }
            UIScreen *s = ((UIWindowScene *)scene).screen;
            if (s == nil || [seen containsObject:s]) {
                continue;
            }
            [seen addObject:s];
            if (n >= cap) {
                return 3;
            }
            double scale = s.scale;
            CGSize size = s.bounds.size;
            if (!isfinite(scale) || scale <= 0.0 || !isfinite(size.width) ||
                !isfinite(size.height)) {
                return 4;
            }
            double w = round(size.width * scale);
            double h = round(size.height * scale);
            if (w <= 0.0 || h <= 0.0 || w > (double)UINT32_MAX || h > (double)UINT32_MAX) {
                return 4;
            }
            FandheScreenRaw *r = &out[n++];
            memset(r, 0, sizeof(*r));
            r->width_px = (uint32_t)w;
            r->height_px = (uint32_t)h;
            r->scale = scale;
            r->is_primary =
                [scene.session.role isEqualToString:UIWindowSceneSessionRoleApplication] ? 1 : 0;
        }
        if (n == 0) {
            return 5;
        }
        qsort(out, n, sizeof(FandheScreenRaw), fandhe_screen_cmp);
        *count = n;
    }
    return 0;
}
