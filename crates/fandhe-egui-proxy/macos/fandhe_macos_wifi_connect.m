// Phase 11 の拡充ループ（親 #165）型サンプル「指定した Wi-Fi への接続」: macOS 実装（Issue #791）。
// CoreWLAN で、指定 SSID への directed scan → SSID のバイト一致する network の選択 →
// `associateToNetwork:password:error:` を行う。接続できるかを事前に判定せず OS の戻り値を写す。
// 失敗したときだけ電源・許可を読み直して理由を分ける。SSID・パスワード・NSError はログに出さない。
//
// `crates/fandhe-egui-proxy/src/device/macos_wifi_connect.rs` から C ABI 経由で呼ばれる。`build.rs` の
// `build_macos()` が feature `device` 有効時にのみコンパイルし、CoreWLAN をリンクする
// （CoreLocation は `fandhe_apple_location.m` のコンパイルでリンク済み）。権限宣言は
// `macos/Info.plist` の `NSLocationUsageDescription`（Rust のテスト実行体には埋め込まれないため、
// テストから実物は呼ばない）。
//
// 契約:
//   int32_t fandhe_macos_wifi_connect(ssid, ssid_len, password, password_len)
//     ワーカースレッドから呼ぶブロッキング関数（main thread で呼ぶと 5 を返す）。許可ダイアログの
//     応答や管理者パスワードのダイアログの間は戻らない（期限を持たない。守らない範囲として Rust 側 doc に
//     明記）。password_len = 0 は暗号なし。
//     戻り値: 0 = 接続した / 1 = 非対応（Wi-Fi インターフェースが無い） /
//             2 = 許可なし（位置情報サービスのオフを含む） / 3 = Wi-Fi の電源がオフ /
//             4 = 見つからない（スキャンが成功して一致 0 件のときだけ。スキャン失敗は 5） / 5 = 内部失敗（引数不正・main thread からの呼び出しを含む） /
//             6 = 対応していない認証方式 / 7 = associate の失敗。
//
// メモリ安全: 渡されたポインタは呼び出しの間だけ使い、保持しない。書き込まない。
// completion は 1 回: 許可の待ち合わせは要求ごとの waiter が持ち、応答を受けた時点で delegate と
// manager を外して 1 回だけ signal する。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。CLLocationManager は main queue で作る。

#import <CoreLocation/CoreLocation.h>
#import <CoreWLAN/CoreWLAN.h>
#import <Foundation/Foundation.h>
#include <stddef.h>
#include <stdint.h>

#define FANDHE_WC_SSID_MAX 32
#define FANDHE_WC_PASSWORD_MAX 64

// 位置情報の許可の応答を 1 回だけ受ける要求ごとの待ち合わせ。manager と delegate の寿命をここに閉じる。
// 別の静的ライブラリに同名の ObjC クラスがあると衝突するため、wifi_info とは別のクラス名にする。
@interface FandheWifiConnectAuthWaiter : NSObject <CLLocationManagerDelegate>
@property(nonatomic, strong) CLLocationManager *manager;
@property(nonatomic, strong) dispatch_semaphore_t semaphore;
@property(nonatomic, assign) BOOL requested;
@property(nonatomic, assign) BOOL done;
@end

@implementation FandheWifiConnectAuthWaiter

// 生成時に 1 回、許可が変わるたびに main thread で呼ばれる。
- (void)locationManagerDidChangeAuthorization:(CLLocationManager *)manager {
    if (self.done) {
        return;
    }
    if (manager.authorizationStatus == kCLAuthorizationStatusNotDetermined) {
        if (!self.requested) {
            self.requested = YES;
            // macOS では WhenInUse でダイアログが出ない（`fandhe_apple_location.m` と同じ事情）。
            [manager requestAlwaysAuthorization];
        }
        return;
    }
    self.done = YES;
    manager.delegate = nil;
    self.manager = nil;
    dispatch_semaphore_signal(self.semaphore);
}

@end

// 位置情報の許可が未決定のときだけ要求し、応答を 1 回待つ。拒否されてもここでは返さない（OS を正にする）。
static void FandheWifiConnectEnsureAuthorization(void) {
    if (![CLLocationManager locationServicesEnabled]) {
        return;
    }
    __block CLAuthorizationStatus status = kCLAuthorizationStatusNotDetermined;
    dispatch_sync(dispatch_get_main_queue(), ^{
        CLLocationManager *probe = [CLLocationManager new];
        status = probe.authorizationStatus;
    });
    if (status != kCLAuthorizationStatusNotDetermined) {
        return;
    }
    FandheWifiConnectAuthWaiter *waiter = [FandheWifiConnectAuthWaiter new];
    waiter.semaphore = dispatch_semaphore_create(0);
    dispatch_async(dispatch_get_main_queue(), ^{
        CLLocationManager *manager = [CLLocationManager new];
        waiter.manager = manager;
        manager.delegate = waiter;
    });
    // 許可ダイアログの応答に期限を持たない（守らない範囲）。
    dispatch_semaphore_wait(waiter.semaphore, DISPATCH_TIME_FOREVER);
}

// 失敗したときの理由の読み直し。電源オフ → 3。`checkPermission` が YES なら、位置情報サービスオフ・
// 許可 Denied/Restricted → 2。それ以外は `fallback`。
static int32_t FandheWifiConnectDiagnose(CWInterface *iface, BOOL checkPermission,
                                         int32_t fallback) {
    if (!iface.powerOn) {
        return 3;
    }
    if (!checkPermission) {
        return fallback;
    }
    if (![CLLocationManager locationServicesEnabled]) {
        return 2;
    }
    __block CLAuthorizationStatus status = kCLAuthorizationStatusNotDetermined;
    dispatch_sync(dispatch_get_main_queue(), ^{
        CLLocationManager *probe = [CLLocationManager new];
        status = probe.authorizationStatus;
    });
    if (status == kCLAuthorizationStatusDenied || status == kCLAuthorizationStatusRestricted) {
        return 2;
    }
    return fallback;
}

// 暗号なし・WPA/WPA2/WPA3 Personal 系だけを扱う。エンタープライズ・WEP 等は扱わない。
static BOOL FandheWifiConnectSupportsPersonal(CWNetwork *net, BOOL open) {
    if (open) {
        return [net supportsSecurity:kCWSecurityNone];
    }
    return [net supportsSecurity:kCWSecurityWPAPersonal] ||
           [net supportsSecurity:kCWSecurityWPA2Personal] ||
           [net supportsSecurity:kCWSecurityWPA3Personal] ||
           [net supportsSecurity:kCWSecurityWPAPersonalMixed];
}

int32_t fandhe_macos_wifi_connect(const uint8_t *ssid, size_t ssid_len, const uint8_t *password,
                                  size_t password_len) {
    if (ssid == NULL || ssid_len == 0 || ssid_len > FANDHE_WC_SSID_MAX || password == NULL ||
        (password_len != 0 && (password_len < 8 || password_len > FANDHE_WC_PASSWORD_MAX))) {
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

        NSData *ssidData = [NSData dataWithBytes:ssid length:ssid_len];
        NSString *ssidString = [[NSString alloc] initWithData:ssidData
                                                      encoding:NSUTF8StringEncoding];
        NSString *passwordString = nil;
        if (password_len != 0) {
            passwordString = [[NSString alloc] initWithBytes:password
                                                      length:password_len
                                                    encoding:NSUTF8StringEncoding];
        }
        if (ssidString == nil || (password_len != 0 && passwordString == nil)) {
            return 5;
        }

        FandheWifiConnectEnsureAuthorization();

        // 指定 SSID への directed scan。SSID のバイト一致するものだけを残す（BSSID は使わない）。
        NSError *scanError = nil;
        NSSet<CWNetwork *> *found = [iface scanForNetworksWithName:ssidString
                                                     includeHidden:YES
                                                             error:&scanError];
        // スキャン自体の失敗（found が nil）は「見つからない」ではない。電源・許可を読み直し、
        // 該当しなければ内部失敗 5 を返す（検索結果ゼロ件と区別する。NSError はログに出さない）。
        if (found == nil) {
            return FandheWifiConnectDiagnose(iface, YES, 5);
        }
        NSMutableArray<CWNetwork *> *matched = [NSMutableArray new];
        for (CWNetwork *net in found) {
            // 許可が無いと ssidData が nil になりうる。照合はバイト一致に限り、一致しない network へは接続しない。
            if (net.ssidData != nil && [net.ssidData isEqualToData:ssidData]) {
                [matched addObject:net];
            }
        }
        if (matched.count == 0) {
            return FandheWifiConnectDiagnose(iface, YES, 4);
        }

        // 一致した中から扱う方式の network を選び、RSSI が最も強いものを使う。
        BOOL open = (password_len == 0);
        CWNetwork *target = nil;
        for (CWNetwork *net in matched) {
            if (!FandheWifiConnectSupportsPersonal(net, open)) {
                continue;
            }
            if (target == nil || net.rssiValue > target.rssiValue) {
                target = net;
            }
        }
        if (target == nil) {
            return 6;
        }

        NSError *associateError = nil;
        BOOL ok = [iface associateToNetwork:target password:passwordString error:&associateError];
        if (ok) {
            return 0;
        }
        return FandheWifiConnectDiagnose(iface, NO, 7);
    }
}
