// 方位（コンパス）: iOS 実装（Issue #1405、親 #1154）。CoreLocation の heading を購読し、
// 最新値を static に保持する（pull 型）。
//
// `src/device/ios_compass.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の feature `device`
// 有効時のみ `cc` コンパイルする（CoreLocation は `apple/fandhe_apple_location.m` 側でリンク済み）。
//
// 契約:
//   fandhe_ios_compass_start() -> int32_t   0 = 成功（冪等）、1 = 非対応（`headingAvailable` が NO。
//                                           シミュレータはここ。何も確保しない）、3 = 失敗
//   fandhe_ios_compass_read(double *degrees, double *accuracy) -> int32_t
//                                           0 = 値なし、1 = 値あり（out へ書く）、2 = 権限拒否、3 = 失敗
//   fandhe_ios_compass_stop()               冪等。保持値と失敗を捨てる
//   delegate が値・失敗を確定した直後に Rust の `fandhe_rust_compass_changed()` を呼ぶ（再描画要求）。
//
// 磁北のみを使い、位置情報の許可は要求しない（Info.plist も変えない）。真北・較正 UI は範囲外。
// `headingAccuracy < 0`（無効サンプル）は値にしない。停止後に届いた通知は `active` で捨てる。
// 保持値は受信から 2 秒を超えたら返さない（途絶は値なし→ドライバの 3 秒期限で NoReading）。
// 測定時刻（CLHeading.timestamp）が購読開始・前景復帰より前のサンプルは捨てる（gSince）。
// 前景復帰は fandhe_rust_compass_resumed で Rust に知らせる。
// 受信時に測定時刻からの古さ age も検査し、age < 0 または age > 2 秒のサンプルは捨てる（遅延配送で古い方位を出さない）。
// CLHeading は測定時刻を壁時計（NSDate）でしか持たないため、age は壁時計どうしの差で求める（同じ時計の 2 点差）。
// 受理後の保持期限は単調時計で測り、起点は「受信時刻 − age」（測定時刻相当）にして、期限の合計を測定から 2 秒に収める。
// 守らない範囲: 時刻変更をまたいだサンプルは値なしに倒れ、次サンプルを待つ（嘘の値は返さない。契約表 3）。
// 背景遷移で保持値を捨て、背景中の通知（heading・失敗とも）は gBackground で拒否する。復帰時にも値を捨てて新規サンプルを待つ。
//
// スレッド: manager の生成・start は main 限定（delegate は生成スレッドの RunLoop で呼ばれる）。
// 購読ごとに manager / delegate を作り、通知は現在の manager と照合して旧購読分を捨てる。
// stop は main 以外なら状態をすぐ捨て、旧 manager の `stopUpdatingHeading` を `dispatch_async` で main へ送る
// （`dispatch_sync` は使わない。`device-bridge-contracts.md`「11」）。状態は os_unfair_lock で守る。
//
// 守る範囲: 嘘の値を出さない・二重購読しない・停止で解除する。
// 守らない範囲: OS 間の値の一致、複数プロセス、較正 UI、表示の遅れ（最大 100 ms + センサー周期）。
// 既知の天井: 許可なしで heading が届くかは実機未確認（届かなければ 3 秒で NoReading。#1403）。
// ログは出さない（方位・精度・NSError を出さない。security.md A09）。

#import <CoreLocation/CoreLocation.h>
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <os/lock.h>
#include <time.h>

// Rust 側 `#[no_mangle]`（再描画要求のみ。引数なし・panic しない）。
extern void fandhe_rust_compass_changed(void);
// 前景復帰の通知。ドライバが NoReading の待機起点を引き直す（描画間隔からは推測しない）。
extern void fandhe_rust_compass_resumed(void);
extern void fandhe_rust_compass_backgrounded(void);

static os_unfair_lock gLock = OS_UNFAIR_LOCK_INIT;
static BOOL gActive = NO;
static BOOL gHasValue = NO;
static BOOL gBackground = NO; // 背景中は通知を捨てる（遷移後・suspend 前に届く通知で古い値が戻るのを防ぐ）
static double gDegrees = 0.0;
static double gAccuracy = 0.0;
static uint64_t gStampNs = 0; // 最後の有効サンプルの測定時刻に相当する単調時刻（ns、受信時刻 − age）。鮮度判定用
// これより古い保持値は返さない（サンプルが途絶えた古い方位を Live のままにしない）。
// `headingFilter = None` で停止中でも継続配信されるので、通常は数十 ms 周期で更新される。
static const uint64_t kStaleNs = 2ULL * NSEC_PER_SEC;
// 購読開始・前景復帰の時刻（壁時計）。これより前の `CLHeading.timestamp` を持つ遅延配送サンプルを拒否する。
// 時刻の前後比較だけに使い、経過時間（鮮度上限）は gStampNs の単調時計で測る（契約表 3）。
// 守らない範囲: システム時刻の変更で比較が誤る場合。拒否されても値なし→次サンプル待ちで、嘘の成功にはならない。
static NSDate *gSince = nil;
static int32_t gFailure = 0; // 0 = なし、2 = 権限拒否、3 = 失敗

@interface FandheIosCompassDelegate : NSObject <CLLocationManagerDelegate>
@end

static CLLocationManager *gManager = nil;
static FandheIosCompassDelegate *gDelegate = nil;
static id gBackgroundToken = nil;
static id gForegroundToken = nil;

@implementation FandheIosCompassDelegate

- (void)locationManager:(CLLocationManager *)manager didUpdateHeading:(CLHeading *)newHeading {
    os_unfair_lock_lock(&gLock);
    if (!gActive || gBackground || manager != gManager) { // 旧購読・背景中の通知は捨てる
        os_unfair_lock_unlock(&gLock);
        return;
    }
    if (newHeading.headingAccuracy < 0) {
        gHasValue = NO;
    } else if (gSince == nil || newHeading.timestamp == nil ||
               [newHeading.timestamp compare:gSince] == NSOrderedAscending) {
        // 開始・復帰より前に測られたサンプル（配送遅れ）は最新値にしない。保持値も出さず次を待つ。
        gHasValue = NO;
    } else {
        // 受信時の古さ。壁時計どうしの差（時刻変更をまたいだ場合のみ誤判定し、値なしに倒れる）。
        NSTimeInterval age = [[NSDate date] timeIntervalSinceDate:newHeading.timestamp];
        if (age < 0 || age > (double)kStaleNs / (double)NSEC_PER_SEC) {
            gHasValue = NO; // 未来・期限超過の測定は捨てて次を待つ
        } else {
            gDegrees = newHeading.magneticHeading;
            gAccuracy = newHeading.headingAccuracy;
            // 起点を測定時刻へ寄せる（age ≤ 2 秒なので現在より前・オーバーフローなし。uptime は 2 秒より大きい前提で差が負になる極短時間のみ 0 に丸める）
            uint64_t now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
            uint64_t ageNs = (uint64_t)(age * (double)NSEC_PER_SEC);
            gStampNs = now > ageNs ? now - ageNs : 0;
            gHasValue = YES;
            gFailure = 0;
        }
    }
    os_unfair_lock_unlock(&gLock);
    fandhe_rust_compass_changed();
}

- (void)locationManager:(CLLocationManager *)manager didFailWithError:(NSError *)error {
    os_unfair_lock_lock(&gLock);
    // 旧購読の失敗は新購読に影響させない。背景中のエラーはアプリ切り替え中の一時的なものなので、
    // heading と同じく捨てる（失敗をラッチすると前面復帰後の read が新サンプルを待たず Failed を返し続ける）。
    if (!gActive || gBackground || manager != gManager) {
        os_unfair_lock_unlock(&gLock);
        return;
    }
    gHasValue = NO;
    BOOL clError = [error.domain isEqualToString:kCLErrorDomain];
    if (clError && error.code == kCLErrorHeadingFailure) {
        // 方位を決められない一時状態。値なしで待ち、3 秒期限は呼び出し側に任せる。
    } else if (clError && error.code == kCLErrorDenied) {
        gFailure = 2;
    } else {
        gFailure = 3;
    }
    os_unfair_lock_unlock(&gLock);
    fandhe_rust_compass_changed();
}

// 較正 UI（OS 標準の八の字ダイアログ）は出さない。
- (BOOL)locationManagerShouldDisplayHeadingCalibration:(CLLocationManager *)manager {
    return NO;
}

@end

int32_t fandhe_ios_compass_start(void) {
    if (![NSThread isMainThread]) {
        return 3;
    }
    @autoreleasepool {
        if (![CLLocationManager headingAvailable]) {
            return 1;
        }
        if (gBackgroundToken == nil) {
            gBackgroundToken = [NSNotificationCenter.defaultCenter
                addObserverForName:UIApplicationDidEnterBackgroundNotification
                            object:nil
                             queue:nil
                        usingBlock:^(NSNotification *__unused note) {
                            os_unfair_lock_lock(&gLock);
                            gBackground = YES;
                            gHasValue = NO;
                            os_unfair_lock_unlock(&gLock);
                            fandhe_rust_compass_backgrounded(); // 背景中は待機を進めさせない
                        }];
            gForegroundToken = [NSNotificationCenter.defaultCenter
                addObserverForName:UIApplicationWillEnterForegroundNotification
                            object:nil
                             queue:nil
                        usingBlock:^(NSNotification *__unused note) {
                            os_unfair_lock_lock(&gLock);
                            gBackground = NO;
                            gSince = [NSDate date];
                            gHasValue = NO; // 新規サンプルの到着を待つ
                            os_unfair_lock_unlock(&gLock);
                            fandhe_rust_compass_resumed(); // 待機の起点を引き直させる
                        }];
        }
        os_unfair_lock_lock(&gLock);
        if (gActive) {
            os_unfair_lock_unlock(&gLock);
            return 0;
        }
        // 購読ごとに manager / delegate を作り直し、通知を現在の manager と照合する。
        gDelegate = [[FandheIosCompassDelegate alloc] init];
        CLLocationManager *m = [[CLLocationManager alloc] init];
        m.delegate = gDelegate;
        m.headingFilter = kCLHeadingFilterNone;
        gManager = m;
        gHasValue = NO;
        gSince = [NSDate date];
        gBackground = UIApplication.sharedApplication.applicationState == UIApplicationStateBackground;
        gFailure = 0;
        gActive = YES;
        os_unfair_lock_unlock(&gLock);
        [m startUpdatingHeading];
    }
    return 0;
}

int32_t fandhe_ios_compass_read(double *degrees, double *accuracy) {
    if (degrees == NULL || accuracy == NULL) {
        return 3;
    }
    int32_t rc = 0;
    os_unfair_lock_lock(&gLock);
    if (gActive) {
        if (gFailure != 0) {
            rc = gFailure;
        } else if (gHasValue && clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - gStampNs <= kStaleNs) {
            *degrees = gDegrees;
            *accuracy = gAccuracy;
            rc = 1;
        }
    }
    os_unfair_lock_unlock(&gLock);
    return rc;
}

void fandhe_ios_compass_stop(void) {
    os_unfair_lock_lock(&gLock);
    CLLocationManager *old = gManager;
    gManager = nil; // 旧 manager の通知はここから照合で捨てられる
    gActive = NO;
    gHasValue = NO;
    gFailure = 0;
    os_unfair_lock_unlock(&gLock);
    if (old == nil) {
        return;
    }
    if ([NSThread isMainThread]) {
        [old stopUpdatingHeading];
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{
            [old stopUpdatingHeading];
        });
    }
}
