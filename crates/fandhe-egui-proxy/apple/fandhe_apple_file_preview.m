// Phase 11 の拡充ループ（親 #182）型サンプル「ファイル・動画のプレビューとサムネイル」:
// macOS（Issue #917）のサムネイル実装。iOS（b-ios）も同じ実装を共用する（QuickLookThumbnailing は
// iOS にもあり、`#if` 分岐は要らない）。
//
// `crates/fandhe-egui-proxy/src/device/apple_file_preview.rs` から C ABI 経由で呼ばれる。
// `build.rs` の macOS / iOS 両分岐が feature `device` 有効時に `cc` でコンパイルし、
// QuickLookThumbnailing と CoreGraphics をリンクする。
//
// 契約（同期呼び出し。ワーカースレッドから呼ぶ。UI・main スレッドは不要）:
//   fandhe_apple_file_thumbnail(path, max_side, timeout_ms, out, out_cap, w, h) -> int32_t
//     path: Rust が組み立てた固定ファイルのパス（NUL 終端。外から任意のパスは来ない）。
//     out/out_cap: Rust が確保した出力バッファ。w*h*4 <= out_cap を確かめてから描く。
//     w/h: 成功時だけ書く。
//     戻り値の表は fandhe_device::file_preview::thumbnail_from_apple_code が SSOT
//     （0 サムネイルあり / 1 非対応 / 2 サムネイルなし / 3 期限切れ / 4 引数・寸法の不正 / 5 その他）。
//
// OS を正にする: `.thumbnail` だけを頼み、返った表現の型も確かめる（アイコンをサムネイルとして
// 成功に混ぜない）。事前判定（拡張子など）はしない。期限が来たら cancelRequest: してから戻る。
// 期限後に completion が来ても Rust のバッファには触れない（箱の中で完結し、画像は解放するだけ）。
//
// 出力は RGBA・行優先・不透明（白で塗ってから描く）。縮小だけを行い max_side に収める。
// 色とアルファの正確さは守らない範囲。CG の資源は単一の出口ですべての経路で解放する。
// パス・画素・エラーはログへ出さない（NSLog 不使用、security.md A09）。
// 実機での表示確認は #890（人手）。

#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <QuickLookThumbnailing/QuickLookThumbnailing.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>

// completion と呼び出し側の受け渡し箱。@synchronized(self) で守る。
@interface FandheThumbBox : NSObject
@property(nonatomic) CGImageRef image;  // 保持している（箱が解放時に手放す）
@property(nonatomic) BOOL done;
@property(nonatomic) BOOL abandoned;  // 呼び出し側が期限で手放した
@property(nonatomic) int32_t code;
@end

@implementation FandheThumbBox
- (void)takeImageInto:(CGImageRef *)dst {
    *dst = _image;
    _image = NULL;
}
- (void)dealloc {
    if (_image != NULL) {
        CGImageRelease(_image);
    }
}
@end

int32_t fandhe_apple_file_thumbnail(const char *path, uint32_t max_side, uint32_t timeout_ms,
                                    uint8_t *out, size_t out_cap, uint32_t *w, uint32_t *h) {
    if (path == NULL || max_side == 0 || out == NULL || w == NULL || h == NULL) {
        return 4;
    }
    if (@available(macOS 10.15, iOS 13.0, *)) {
        @autoreleasepool {
            NSString *p = [NSString stringWithUTF8String:path];
            if (p == nil) {
                return 4;
            }
            NSURL *url = [NSURL fileURLWithPath:p];
            QLThumbnailGenerationRequest *req = [[QLThumbnailGenerationRequest alloc]
                  initWithFileAtURL:url
                               size:CGSizeMake(max_side, max_side)
                              scale:1.0
                representationTypes:QLThumbnailGenerationRequestRepresentationTypeThumbnail];
            FandheThumbBox *box = [[FandheThumbBox alloc] init];
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            [[QLThumbnailGenerator sharedGenerator]
                generateBestRepresentationForRequest:req
                                   completionHandler:^(QLThumbnailRepresentation *_Nullable rep,
                                                       NSError *_Nullable err) {
                                       @synchronized(box) {
                                           if (box.abandoned) {
                                               return;  // 期限後。何も保持しない
                                           }
                                           if (rep != nil) {
                                               if (rep.type == QLThumbnailRepresentationTypeThumbnail &&
                                                   rep.CGImage != NULL) {
                                                   box.image = CGImageRetain(rep.CGImage);
                                                   box.code = 0;
                                               } else {
                                                   box.code = 2;  // アイコン等はサムネイルではない
                                               }
                                           } else if (err != nil &&
                                                      [err.domain isEqualToString:QLThumbnailErrorDomain] &&
                                                      err.code == QLThumbnailErrorGenerationFailed) {
                                               box.code = 2;
                                           } else {
                                               box.code = 5;
                                           }
                                           box.done = YES;
                                       }
                                       dispatch_semaphore_signal(sem);
                                   }];
            dispatch_semaphore_wait(
                sem, dispatch_time(DISPATCH_TIME_NOW, (int64_t)timeout_ms * (int64_t)NSEC_PER_MSEC));

            CGImageRef img = NULL;
            int32_t code = 5;
            BOOL timed_out = NO;
            @synchronized(box) {
                if (box.done) {
                    code = box.code;
                    [box takeImageInto:&img];
                } else {
                    box.abandoned = YES;
                    timed_out = YES;
                }
            }
            if (timed_out) {
                [[QLThumbnailGenerator sharedGenerator] cancelRequest:req];
                return 3;
            }
            if (code != 0 || img == NULL) {
                if (img != NULL) {
                    CGImageRelease(img);
                }
                return code == 0 ? 5 : code;
            }

            int32_t result = 5;
            CGColorSpaceRef space = NULL;
            CGContextRef ctx = NULL;
            size_t iw = CGImageGetWidth(img);
            size_t ih = CGImageGetHeight(img);
            if (iw == 0 || ih == 0) {
                result = 4;
                goto done;
            }
            // 縮小だけ。長辺を max_side に収める（拡大はしない）。
            double scale = 1.0;
            double longest = (double)(iw > ih ? iw : ih);
            if (longest > (double)max_side) {
                scale = (double)max_side / longest;
            }
            double fw = round((double)iw * scale);
            double fh = round((double)ih * scale);
            uint32_t pw = fw < 1 ? 1 : (fw > max_side ? max_side : (uint32_t)fw);
            uint32_t ph = fh < 1 ? 1 : (fh > max_side ? max_side : (uint32_t)fh);
            if ((size_t)pw * (size_t)ph * 4 > out_cap) {
                result = 4;
                goto done;
            }
            space = CGColorSpaceCreateDeviceRGB();
            if (space == NULL) {
                goto done;
            }
            ctx = CGBitmapContextCreate(out, pw, ph, 8, (size_t)pw * 4, space,
                                        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
            if (ctx == NULL) {
                goto done;
            }
            CGContextSetRGBFillColor(ctx, 1, 1, 1, 1);
            CGContextFillRect(ctx, CGRectMake(0, 0, pw, ph));
            CGContextDrawImage(ctx, CGRectMake(0, 0, pw, ph), img);
            *w = pw;
            *h = ph;
            result = 0;

        done:
            if (ctx != NULL) {
                CGContextRelease(ctx);
            }
            if (space != NULL) {
                CGColorSpaceRelease(space);
            }
            CGImageRelease(img);
            return result;
        }
    }
    return 1;
}
