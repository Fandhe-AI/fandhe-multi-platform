// Phase 11 の拡充ループ（親 #133）型サンプル「住所の座標変換」: macOS（Issue #691）と iOS
// （#692）の実装。macOS / iOS は実装本体を共用する。それ以外（visionOS 等。build.rs からは
// 呼ばれない）は常に Unsupported のスタブ。
//
// `crates/fandhe-egui-proxy/src/device/apple_geocoding_search.rs` から C ABI 経由で呼ばれる。
// `build.rs` の macOS / iOS 両分岐が feature `device` 有効時に `cc` でコンパイルし、MapKit と
// CoreLocation をリンクする。MapKit `MKGeocodingRequest`（26 以上）で固定の住所 1 件を座標へ変換する。
// 26 未満は非推奨の CLGeocoder へ倒さず Unsupported を返す。
//
// 契約（同期呼び出し。呼び出し元は専用ワーカーで呼ぶ）:
//   fandhe_apple_geocoding_search_forward(addr, len, deadline_ms, out_lat, out_lon, out_has_coord,
//                                         out_count) -> int32_t
//     addr は UTF-8 の住所（呼び出しの間だけ有効。保持しない）。deadline_ms は残り時間。
//     戻り値の表（device/apple_geocoding_search.rs の match が SSOT）:
//       0 成功 / 1 非対応（26 未満） / 2 通信不可 / 3 回数上限 / 4 該当なし / 5 期限切れ /
//       6 入力不正 / 7 main スレッドから呼ばれた / 8 それ以外の OS の失敗。
//     out_* への書き込みは戻り値 0 のときだけ、待機明けにワーカー自身が行う。期限切れで戻った後に
//     Rust の領域へは書かない（箱だけを触る）。
//
// スレッド: MKGeocodingRequest の開始は main queue、completion も main で届く（Apple の文書）。
// ワーカーが main で待つとデッドロックするため main からの呼び出しは 7 で拒否する。completion と
// 期限切れの競合は箱（@synchronized）の短い区間だけで捌き、その内側で dispatch_sync しない
// （契約表 11）。各入口は @autoreleasepool で包む（契約表 5）。
// 資源の解放（契約表 9）: 要求オブジェクトは completion・期限切れ（main で cancel）・生成失敗の
// 全経路で箱から外す。error が付いた結果は件数があっても失敗にする（嘘の成功の禁止）。
//
// 権限・Info.plist のキー・CoreLocation の権限は使わない。住所・座標・NSError の内容は
// ログへ出さない（NSLog 不使用、security.md A09）。実機での通信・精度の確認は #689（人手）。

#import <Foundation/Foundation.h>
#import <TargetConditionals.h>

#if TARGET_OS_OSX || TARGET_OS_IOS

#import <CoreLocation/CoreLocation.h>
#import <MapKit/MapKit.h>

// FFI 側の住所の長さ上限（Rust 側 `MAX_ADDRESS_BYTES` より緩い防御的 guard）。
static const size_t kFandheGeocodeMaxBytes = 4096;
// dispatch_time へ渡す期限の上限（ms）。乗算の桁あふれ防止。
static const uint64_t kFandheGeocodeMaxDeadlineMs = 10 * 60 * 1000;

// 通信不可の判定。`fandhe_apple_map_view.m` の `FandheIsOfflineError` と同じ内容（static のため
// 共有せず複製している。変更時は両方を揃える）。
static BOOL FandheGeocodeIsOfflineError(NSError *error) {
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

// NSError を戻りコードへ写す（2 通信不可 / 3 回数上限 / 4 該当なし / 8 それ以外）。
static int32_t FandheGeocodeErrorCode(NSError *error) {
    if (FandheGeocodeIsOfflineError(error)) {
        return 2;
    }
    if ([error.domain isEqualToString:MKErrorDomain]) {
        switch (error.code) {
        case MKErrorLoadingThrottled:
            return 3;
        case MKErrorPlacemarkNotFound:
            return 4;
        case MKErrorServerFailure:
        case MKErrorUnknown: {
            id underlying = error.userInfo[NSUnderlyingErrorKey];
            if ([underlying isKindOfClass:[NSError class]] &&
                FandheGeocodeIsOfflineError((NSError *)underlying)) {
                return 2;
            }
            return 8;
        }
        default:
            return 8;
        }
    }
    return 8;
}

// ワーカーと main の間で結果を受け渡す箱。フィールドの読み書きは @synchronized(self) の内側だけ。
@interface FandheGeocodeBox : NSObject
@property(nonatomic, strong) dispatch_semaphore_t sem;
@property(nonatomic) BOOL finished;
@property(nonatomic) BOOL timedOut;
@property(nonatomic) int32_t code;
@property(nonatomic) double lat;
@property(nonatomic) double lon;
@property(nonatomic) BOOL hasCoord;
@property(nonatomic) uint32_t count;
// 進行中の要求（main からのみ設定・解放する）。
@property(nonatomic, strong) id request;
@end

@implementation FandheGeocodeBox
@end

// 結果を確定して待機中のワーカーを起こす。期限切れ確定済みなら何もしない。
static void FandheGeocodeFinish(FandheGeocodeBox *box, int32_t code, double lat, double lon,
                                BOOL hasCoord, uint32_t count) {
    @synchronized(box) {
        box.request = nil;
        if (box.timedOut) {
            return;
        }
        box.finished = YES;
        box.code = code;
        box.lat = lat;
        box.lon = lon;
        box.hasCoord = hasCoord;
        box.count = count;
    }
    dispatch_semaphore_signal(box.sem);
}

API_AVAILABLE(macos(26.0), ios(26.0))
static void FandheGeocodeStart(FandheGeocodeBox *box, NSString *address) {
    MKGeocodingRequest *request = [[MKGeocodingRequest alloc] initWithAddressString:address];
    if (request == nil) {
        FandheGeocodeFinish(box, 6, 0, 0, NO, 0);
        return;
    }
    @synchronized(box) {
        if (box.timedOut) {
            return;
        }
        box.request = request;
    }
    [request getMapItemsWithCompletionHandler:^(NSArray<MKMapItem *> *items, NSError *error) {
      @autoreleasepool {
          if (error != nil) {
              FandheGeocodeFinish(box, FandheGeocodeErrorCode(error), 0, 0, NO, 0);
              return;
          }
          if (items.count == 0) {
              FandheGeocodeFinish(box, 4, 0, 0, NO, 0);
              return;
          }
          uint32_t count = items.count > UINT32_MAX ? UINT32_MAX : (uint32_t)items.count;
          CLLocation *location = items.firstObject.location;
          BOOL has = location != nil && CLLocationCoordinate2DIsValid(location.coordinate);
          FandheGeocodeFinish(box, 0, has ? location.coordinate.latitude : 0,
                              has ? location.coordinate.longitude : 0, has, count);
      }
    }];
}

int32_t fandhe_apple_geocoding_search_forward(const uint8_t *addr, size_t len,
                                              uint64_t deadline_ms, double *out_lat,
                                              double *out_lon, uint8_t *out_has_coord,
                                              uint32_t *out_count) {
    @autoreleasepool {
        if (addr == NULL || len == 0 || len > kFandheGeocodeMaxBytes || out_lat == NULL ||
            out_lon == NULL || out_has_coord == NULL || out_count == NULL) {
            return 6;
        }
        NSString *address = [[NSString alloc] initWithBytes:addr
                                                     length:len
                                                   encoding:NSUTF8StringEncoding];
        if (address == nil) {
            return 6;
        }
        // completion は main で届くため、main で待つとデッドロックする。
        if ([NSThread isMainThread]) {
            return 7;
        }
        if (@available(macOS 26.0, iOS 26.0, *)) {
            FandheGeocodeBox *box = [[FandheGeocodeBox alloc] init];
            box.sem = dispatch_semaphore_create(0);
            dispatch_async(dispatch_get_main_queue(), ^{
              @autoreleasepool {
                  FandheGeocodeStart(box, address);
              }
            });
            uint64_t ms = deadline_ms > kFandheGeocodeMaxDeadlineMs ? kFandheGeocodeMaxDeadlineMs
                                                                     : deadline_ms;
            long waited = dispatch_semaphore_wait(
                box.sem, dispatch_time(DISPATCH_TIME_NOW, (int64_t)(ms * NSEC_PER_MSEC)));
            @synchronized(box) {
                if (waited != 0 && !box.finished) {
                    // 期限切れ。以後の completion は箱に書かず捨てる。
                    box.timedOut = YES;
                } else {
                    // 待機明け（期限ぎりぎりに確定した結果も含む）。ワーカー自身がコピーする。
                    *out_lat = box.lat;
                    *out_lon = box.lon;
                    *out_has_coord = box.hasCoord ? 1 : 0;
                    *out_count = box.count;
                    return box.code;
                }
            }
            // main 側で cancel して参照を外す。ロックの内側では dispatch しない。
            dispatch_async(dispatch_get_main_queue(), ^{
              @autoreleasepool {
                  id pending = nil;
                  @synchronized(box) {
                      pending = box.request;
                      box.request = nil;
                  }
                  if (pending != nil) {
                      [(MKGeocodingRequest *)pending cancel];
                  }
              }
            });
            return 5;
        }
        return 1;
    }
}

#else

int32_t fandhe_apple_geocoding_search_forward(const uint8_t *addr, size_t len,
                                              uint64_t deadline_ms, double *out_lat,
                                              double *out_lon, uint8_t *out_has_coord,
                                              uint32_t *out_count) {
    (void)addr;
    (void)len;
    (void)deadline_ms;
    (void)out_lat;
    (void)out_lon;
    (void)out_has_coord;
    (void)out_count;
    return 1;
}

#endif
