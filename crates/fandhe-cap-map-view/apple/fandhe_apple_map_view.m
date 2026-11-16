// Phase 11 の拡充ループ（親 #85）型サンプル「地図表示」: macOS（Issue #273）と iOS（Issue #274）の実装。
// macOS / iOS は実装本体を共用し、画像型・CGImage の取り出し・scale の指定だけ小さな #if で分ける。
// それ以外（visionOS 等。build.rs からは呼ばれない）は常に Unsupported のスタブ。
//
// `crates/fandhe-egui-proxy/src/device/apple_map_view.rs` から C ABI 経由で呼ばれる。
// `build.rs` の macOS / iOS 両分岐が feature `device` 有効時に `cc` でコンパイルし、MapKit と
// CoreGraphics をリンクする。
//
// 契約（pull 型。`fandhe_apple_photo_library.m` と同型）:
//   fandhe_apple_map_view_start(request_id, lat, lon, span_m, w, h) -> int32_t
//     戻り値: 0 = 受理（結果は非同期に fandhe_rust_map_view_outcome で届く）、
//     1 = 非対応、5 = main スレッド以外から呼ばれた。受理しなかった場合は outcome を呼ばない。
//   fandhe_apple_map_view_cancel(request_id)
//     id が進行中のものと一致するときだけ取り消して参照を解放する。
//   fandhe_rust_map_view_outcome(request_id, code, rgba, len, w, h)
//     Rust 側が公開する関数。code の表は device/map_view.rs::outcome_from_apple_code が SSOT
//     （0 画像あり / 1 非対応 / 2 通信できない / 3 MapKit の失敗 / 4 画像なし・変換失敗 /
//      5 main 以外）。rgba は呼び出しの間だけ有効（Rust 側がコピーし、こちらが直後に free する）。
//     受理した各依頼につき高々 1 回呼ぶ（cancel 後は呼ばない場合がある。期限は Rust 側が持つ）。
//
// スレッド: MKMapSnapshotter の completion は main で呼ばれる（Apple の文書）。入口も main に限る
// ため、ロックも dispatch_sync も使わない（契約表 11）。各入口は @autoreleasepool で包む
// （契約表 5）。error が付いた snapshot は画像があっても失敗にする（嘘の成功の禁止）。
//
// iOS: 背景にいる間は completion の配送が前景復帰まで遅れる。その場合は Rust 側の 15 秒の期限で
// 失敗が確定し、復帰後に届いた completion は id 照合で捨てる。scale は 1x（traitCollection）に
// 固定する（ポイントとピクセルを揃えて余分な画素を取らない。RGBA は常に w x h の context へ描く）。
//
// 権限・CoreLocation は使わない。座標・NSError の内容・画素はログへ出さない（NSLog 不使用、
// security.md A09）。実機での表示確認（タイルが取れるか・上下の向き）は #272（人手）。

#import <Foundation/Foundation.h>
#import <TargetConditionals.h>

extern void fandhe_rust_map_view_outcome(uint64_t request_id, int32_t code, const uint8_t *rgba,
                                         size_t len, uint32_t w, uint32_t h);

#if TARGET_OS_OSX || TARGET_OS_IOS

#if TARGET_OS_OSX
#import <AppKit/AppKit.h>
typedef NSImage FandhePlatformImage;
#else
#import <UIKit/UIKit.h>
typedef UIImage FandhePlatformImage;
#endif
#import <CoreGraphics/CoreGraphics.h>
#import <MapKit/MapKit.h>

// 進行中の要求（main スレッドからのみ触る）。
static MKMapSnapshotter *gCurrent = nil;
static uint64_t gCurrentId = 0;

static BOOL FandheIsOfflineError(NSError *error) {
    if (![error.domain isEqualToString:NSURLErrorDomain]) {
        return NO;
    }
    switch (error.code) {
    case NSURLErrorNotConnectedToInternet:
    case NSURLErrorNetworkConnectionLost:
    case NSURLErrorDataNotAllowed:
    case NSURLErrorInternationalRoamingOff:
        return YES;
    default:
        return NO;
    }
}

static CGImageRef FandheCGImageOf(FandhePlatformImage *image) {
#if TARGET_OS_OSX
    return [image CGImageForProposedRect:NULL context:nil hints:nil];
#else
    return image.CGImage;
#endif
}

// スナップショット画像を w x h の RGBA8 に描く（premultiplied だが地図は不透明で、非乗算アルファと同じ値になる）。
// 失敗は NULL。成功時は呼び出し側が free する。全経路で CG 資源を解放する。
static uint8_t *FandheRenderRGBA(FandhePlatformImage *image, uint32_t w, uint32_t h) {
    CGImageRef cg = FandheCGImageOf(image);
    if (cg == NULL) {
        return NULL;
    }
    size_t len = (size_t)w * (size_t)h * 4;
    uint8_t *buf = calloc(len, 1);
    if (buf == NULL) {
        return NULL;
    }
    CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
    if (space == NULL) {
        free(buf);
        return NULL;
    }
    CGContextRef ctx = CGBitmapContextCreate(buf, w, h, 8, (size_t)w * 4, space,
                                             kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
    CGColorSpaceRelease(space);
    if (ctx == NULL) {
        free(buf);
        return NULL;
    }
    CGContextDrawImage(ctx, CGRectMake(0, 0, w, h), cg);
    CGContextRelease(ctx);
    return buf;
}

int32_t fandhe_apple_map_view_start(uint64_t request_id, double lat, double lon, double span_m,
                                    uint32_t w, uint32_t h) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 5;
        }
        // 新しい要求が来たら前のものを取り消して差し替える。
        [gCurrent cancel];
        gCurrent = nil;

        MKMapSnapshotOptions *options = [[MKMapSnapshotOptions alloc] init];
        options.region = MKCoordinateRegionMakeWithDistance(CLLocationCoordinate2DMake(lat, lon),
                                                            span_m, span_m);
        options.size = CGSizeMake(w, h);
#if TARGET_OS_IOS
        options.traitCollection = [UITraitCollection traitCollectionWithDisplayScale:1.0];
#endif
        MKMapSnapshotter *snapshotter = [[MKMapSnapshotter alloc] initWithOptions:options];
        gCurrent = snapshotter;
        gCurrentId = request_id;

        [snapshotter startWithCompletionHandler:^(MKMapSnapshot *_Nullable snapshot,
                                                   NSError *_Nullable error) {
          @autoreleasepool {
              // 取り消し済み・差し替え済みの完了は捨てる。
              if (gCurrentId != request_id || gCurrent != snapshotter) {
                  return;
              }
              gCurrent = nil;
              if (error != nil) {
                  fandhe_rust_map_view_outcome(request_id, FandheIsOfflineError(error) ? 2 : 3, NULL,
                                               0, 0, 0);
                  return;
              }
              if (snapshot == nil || snapshot.image == nil) {
                  fandhe_rust_map_view_outcome(request_id, 4, NULL, 0, 0, 0);
                  return;
              }
              uint8_t *rgba = FandheRenderRGBA(snapshot.image, w, h);
              if (rgba == NULL) {
                  fandhe_rust_map_view_outcome(request_id, 4, NULL, 0, 0, 0);
                  return;
              }
              fandhe_rust_map_view_outcome(request_id, 0, rgba, (size_t)w * (size_t)h * 4, w, h);
              free(rgba);
          }
        }];
        return 0;
    }
}

void fandhe_apple_map_view_cancel(uint64_t request_id) {
    @autoreleasepool {
        if (![NSThread isMainThread] || gCurrentId != request_id) {
            return;
        }
        [gCurrent cancel];
        gCurrent = nil;
    }
}

#else

// macOS / iOS 以外（build.rs からは呼ばれない）。常に非対応を返し、成功を作らない。
int32_t fandhe_apple_map_view_start(uint64_t request_id, double lat, double lon, double span_m,
                                    uint32_t w, uint32_t h) {
    (void)request_id;
    (void)lat;
    (void)lon;
    (void)span_m;
    (void)w;
    (void)h;
    return 1;
}

void fandhe_apple_map_view_cancel(uint64_t request_id) { (void)request_id; }

#endif
