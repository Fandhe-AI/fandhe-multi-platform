// Phase 11 の拡充ループ（親 #111）型サンプル「Wi-Fi 情報」: macOS 実装（Issue #514）。
// CoreWLAN の `CWWiFiClient` で、今つないでいる Wi-Fi の SSID と電波強度、OS が持つ直近のスキャン結果
// （`cachedScanResults`）を読む。macOS 14 以降は SSID の取得に位置情報の許可が要るため、読む前に
// CoreLocation で許可を判定する。**自分からスキャンしない**。BSSID は読まない。SSID・NSError は
// ログに出さない。
//
// `crates/fandhe-egui-proxy/src/device/macos_wifi_info.rs` から C ABI 経由で呼ばれる。`build.rs` の
// `build_macos()` が feature `device` 有効時にのみコンパイルし、CoreWLAN をリンクする
// （CoreLocation は `fandhe_apple_location.m` のコンパイルでリンク済み）。権限宣言は
// `macos/Info.plist` の `NSLocationUsageDescription`（Rust のテスト実行体には埋め込まれないため、
// テストから実物は呼ばない）。
//
// 契約:
//   typedef struct { uint8_t ssid[32]; int32_t ssid_len; int32_t rssi; } FandheWifiNet;
//   fandhe_macos_wifi_info_read(current, has_current, nearby, cap, nearby_len)
//     ワーカースレッドから呼ぶブロッキング関数（main thread で呼ぶと 5 を返す）。許可ダイアログの
//     応答を待つ間は戻らない（期限を持たない。守らない範囲として Rust 側 doc に明記）。
//     戻り値: 0 = OK（未接続・周辺 0 件も正当な成功）/ 1 = 非対応（Wi-Fi インターフェースが無い）/
//             2 = 位置情報の許可なし / 3 = Wi-Fi の電源がオフ / 4 = 位置情報サービスがオフ /
//             5 = 内部失敗（引数不正・main thread からの呼び出しを含む）。
//             0 以外のとき out_* は書き換えない。
//     成功時、*has_current は接続中なら 1。current は接続中のときだけ書く。nearby へ SSID ごとに最強の 1 件へ畳み
//     強い順に `cap` 件を上限に書き、*nearby_len に件数を入れる。正規化（並べ替え・上位 5 件・
//     値域の検証）は Rust 側で行う。ssid_len = -1 は SSID 無し（nil・32 バイト超）。rssi は OS の
//     値のまま（0 は未取得）。
//
// メモリ安全: 渡されたポインタは呼び出しの間だけ使い、保持しない。nearby へ書くのは `cap` 件まで。
// completion は 1 回: 許可の待ち合わせは要求ごとの waiter が持ち、応答を受けた時点で delegate と
// manager を外して 1 回だけ signal する。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。CLLocationManager は main queue で作る。

#import <CoreLocation/CoreLocation.h>
#import <CoreWLAN/CoreWLAN.h>
#import <Foundation/Foundation.h>
#include <stddef.h>
#include <stdint.h>

#define FANDHE_SSID_MAX 32

typedef struct {
    uint8_t ssid[FANDHE_SSID_MAX];
    int32_t ssid_len;
    int32_t rssi;
} FandheWifiNet;

// 位置情報の許可の応答を 1 回だけ受ける要求ごとの待ち合わせ。manager と delegate の寿命をここに閉じる。
@interface FandheWifiAuthWaiter : NSObject <CLLocationManagerDelegate>
@property(nonatomic, strong) CLLocationManager *manager;
@property(nonatomic, strong) dispatch_semaphore_t semaphore;
@property(nonatomic, assign) BOOL requested;
@property(nonatomic, assign) BOOL done;
@property(nonatomic, assign) CLAuthorizationStatus status;
@end

@implementation FandheWifiAuthWaiter

// 生成時に 1 回、許可が変わるたびに main thread で呼ばれる。
- (void)locationManagerDidChangeAuthorization:(CLLocationManager *)manager {
    if (self.done) {
        return;
    }
    CLAuthorizationStatus status = manager.authorizationStatus;
    if (status == kCLAuthorizationStatusNotDetermined) {
        if (!self.requested) {
            self.requested = YES;
            // macOS では WhenInUse でダイアログが出ない（`fandhe_apple_location.m` と同じ事情）。
            [manager requestAlwaysAuthorization];
        }
        return;
    }
    self.done = YES;
    self.status = status;
    manager.delegate = nil;
    self.manager = nil;
    dispatch_semaphore_signal(self.semaphore);
}

@end

// SSID の生バイトを書く。nil・32 バイト超は ssid_len = -1。
static void FandheFillNet(FandheWifiNet *out, NSData *ssidData, NSInteger rssi) {
    memset(out, 0, sizeof(*out));
    if (ssidData != nil && ssidData.length <= FANDHE_SSID_MAX) {
        out->ssid_len = (int32_t)ssidData.length;
        memcpy(out->ssid, ssidData.bytes, ssidData.length);
    } else {
        out->ssid_len = -1;
    }
    if (rssi > INT32_MAX) {
        rssi = INT32_MAX;
    } else if (rssi < INT32_MIN) {
        rssi = INT32_MIN;
    }
    out->rssi = (int32_t)rssi;
}

// 比較用の強さ。契約の値域（-126..=-1 dBm）外は最弱。
static NSInteger FandheRank(NSInteger rssi) {
    return (rssi >= -126 && rssi <= -1) ? rssi : NSIntegerMin;
}

int32_t fandhe_macos_wifi_info_read(FandheWifiNet *current, int32_t *has_current,
                                    FandheWifiNet *nearby, size_t cap, size_t *nearby_len) {
    if (current == NULL || has_current == NULL || nearby == NULL || nearby_len == NULL) {
        return 5;
    }
    @autoreleasepool {
        // 許可の応答待ちで main thread を塞がない。
        if ([NSThread isMainThread]) {
            return 5;
        }
        CWInterface *iface = [CWWiFiClient sharedWiFiClient].interface;
        if (iface == nil) {
            return 1;
        }
        if (![CLLocationManager locationServicesEnabled]) {
            return 4;
        }

        FandheWifiAuthWaiter *waiter = [FandheWifiAuthWaiter new];
        waiter.semaphore = dispatch_semaphore_create(0);
        dispatch_async(dispatch_get_main_queue(), ^{
            CLLocationManager *manager = [CLLocationManager new];
            waiter.manager = manager;
            manager.delegate = waiter;
        });
        // ponytail: 許可ダイアログの応答に期限を持たない。期限を付けるなら Rust 側の `recv_timeout` と
        // 同じ値を引数で受け、期限切れ後に waiter を捨てる。
        dispatch_semaphore_wait(waiter.semaphore, DISPATCH_TIME_FOREVER);
        CLAuthorizationStatus status = waiter.status;
        if (status == kCLAuthorizationStatusDenied || status == kCLAuthorizationStatusRestricted) {
            return 2;
        }
        if (status != kCLAuthorizationStatusAuthorizedAlways) {
            return 5;
        }

        if (!iface.powerOn) {
            return 3;
        }

        // 接続中は station モード。未接続（モードなし）は has_current = 0。
        if (iface.interfaceMode == kCWInterfaceModeStation) {
            FandheFillNet(current, iface.ssidData, iface.rssiValue);
            *has_current = 1;
        } else {
            *has_current = 0;
        }

        // OS が持つ直近のスキャン結果。cap で切る前に SSID（生バイト）ごとへ最強の 1 件に畳み、
        // 強い順に並べる（自分からはスキャンしない）。同一 SSID の多数の AP が上限を埋めて、別 SSID が
        // 落ちるのを防ぐ。rssi 0（未取得）などの範囲外は最弱として扱う。
        NSMutableDictionary<id, CWNetwork *> *best = [NSMutableDictionary new];
        for (CWNetwork *net in iface.cachedScanResults) {
            id key = net.ssidData ?: (id)[NSNull null];
            CWNetwork *old = best[key];
            if (old == nil || FandheRank(net.rssiValue) > FandheRank(old.rssiValue)) {
                best[key] = net;
            }
        }
        NSArray<CWNetwork *> *sorted = [best.allValues
            sortedArrayUsingComparator:^NSComparisonResult(CWNetwork *a, CWNetwork *b) {
                NSInteger ra = FandheRank(a.rssiValue), rb = FandheRank(b.rssiValue);
                if (ra == rb) {
                    return NSOrderedSame;
                }
                return ra > rb ? NSOrderedAscending : NSOrderedDescending;
            }];
        size_t n = 0;
        for (CWNetwork *net in sorted) {
            if (n >= cap) {
                break;
            }
            FandheFillNet(&nearby[n], net.ssidData, net.rssiValue);
            n++;
        }
        *nearby_len = n;
        return 0;
    }
}
