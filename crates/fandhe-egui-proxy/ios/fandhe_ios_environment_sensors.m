// 気圧・近接センサー: iOS 実装（Issue #610、親 #123）。CoreMotion `CMAltimeter` の気圧と
// `UIDevice` の近接監視を購読し、最新値を static に保持する（pull 型）。照度は公開 API が無いので
// このファイルでは扱わない（Rust 側が `UnsupportedOnOs` を返す）。
//
// `src/device/ios_environment_sensors.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（CoreMotion・UIKit は他の .m 用にリンク済み）。
//
// 契約（気圧）:
//   fandhe_ios_env_pressure_start() -> int32_t   0 = 成功（冪等）、1 = 端末非搭載（シミュレータ）、
//                                                2 = 権限拒否（Denied / Restricted）、3 = 失敗
//   fandhe_ios_env_pressure_read(double *kpa, double *age_ms) -> int32_t
//                                                0 = 値なし、1 = 値あり（out へ書く）、2 = 権限拒否、3 = 失敗
//                                                age_ms は単調時計で測った測定時刻からの経過
//   fandhe_ios_env_pressure_stop()               冪等。世代を進め、保持値と失敗を捨てる
// 契約（近接）:
//   fandhe_ios_env_proximity_start() -> int32_t  0 = 成功（冪等）、1 = 端末非搭載（有効にしても NO のまま）、
//                                                3 = 失敗（main 以外から呼んだ場合を含む）
//   fandhe_ios_env_proximity_read(int32_t *near) -> int32_t
//                                                0 = 値なし、1 = 値あり（1 = 近い / 0 = 遠い）、3 = 失敗
//   fandhe_ios_env_proximity_stop()              冪等。main 以外なら main queue へ非同期に送る
// 値・失敗を確定した直後に Rust の `fandhe_rust_environment_sensors_changed()` を呼ぶ（再描画要求。契約表 4）。
//
// 気圧: 購読ごとに CMAltimeter と世代番号を作り、handler は自分の世代を現在と照合して旧購読の通知を捨てる。
// 拒否コードは `CMErrorNotAuthorized` / `CMErrorMotionActivityNotAuthorized` の 2 種とも 2 に写し、
// それ以外は 3 にする（元のコードを潰さない）。サンプルの測定時刻（CMLogItem.timestamp、起動からの秒）は
// `NSProcessInfo.systemUptime` と同じ時計なので、その差を古さとする。開始・復帰より前の測定と、
// 古さが 2 秒を超えるサンプルは捨てる。経過の判定に壁時計（NSDate）は使わない（契約表 3）。
//
// 近接: 有効化前の `proximityMonitoringEnabled` を保存し、停止で復元する（共有状態の復元。契約表 9）。
// 有効化・復元・observer の解除は main 限定。on-change 通知は状態が変わるまで来ないので、
// 開始時点の `proximityState` を初期値として保持する。observer のトークンは static に握る（契約表 1）。
// stop が main 以外なら状態をすぐ捨て、復元を `dispatch_async` で main へ送る（`dispatch_sync` は使わない。
// 契約表 11）。送ったブロックは世代を照合し、その間に再開されていれば何もしない。
//
// ライフサイクル（気圧・近接共用。observer は 1 回だけ登録）: 背景遷移で両方の保持値を捨て、気圧の
// CMAltimeter 更新を止め、近接の observer を外して proximityMonitoringEnabled を有効化前の値へ戻す。
// 希望状態（gPressureActive / gProximityActive）は残し、前景復帰で希望されている購読だけを再開する
// （気圧は新しい世代で、近接は再有効化と observer の再登録）。背景中の通知は gBackground でも拒否する。
// 失敗の保持（gPressureFailure）は購読の世代に属する。新しい世代の開始・背景遷移・停止で 0 に戻し、
// handler は世代照合の後でのみ書く。
// 許可ダイアログは Inactive になるだけで背景へは遷移しないので、ダイアログの応答が背景フラグと競合して
// 正常な許可を中断扱いにすることはない。
//
// 守る範囲: 嘘の値を出さない・二重購読しない・停止と背景で保持値を捨てる。
// 守らない範囲: 絶対精度、近接で画面が消えること、複数プロセス、表示の遅れ。
// 既知の天井: 近接の初期値と気圧の配信周期は実機未確認（#602）。
// ログは出さない（気圧・近接・NSError を出さない。security.md A09）。

#import <CoreMotion/CoreMotion.h>
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <os/lock.h>
#include <math.h>
#include <time.h>

// Rust 側 `#[no_mangle]`（引数なし・panic しない）。
extern void fandhe_rust_environment_sensors_changed(void);
extern void fandhe_rust_environment_sensors_backgrounded(void);
extern void fandhe_rust_environment_sensors_resumed(void);

static os_unfair_lock gLock = OS_UNFAIR_LOCK_INIT;
static BOOL gBackground = NO;
static id gBackgroundToken = nil;
static id gForegroundToken = nil;

// 気圧
static const double kPressureStaleSec = 2.0;
static CMAltimeter *gAltimeter = nil;
static NSOperationQueue *gPressureQueue = nil;
static uint64_t gPressureGen = 0;
static BOOL gPressureActive = NO;
static BOOL gPressureHasValue = NO;
static double gPressureKpa = 0.0;
static uint64_t gPressureStampNs = 0; // 測定時刻に相当する単調時刻（ns）
static double gPressureSince = 0.0;   // 受付起点（systemUptime 秒）。これより前の測定は捨てる
static int32_t gPressureFailure = 0;  // 0 = なし、2 = 権限拒否、3 = 失敗

// 近接
static uint64_t gProximityGen = 0;
static BOOL gProximityActive = NO;
static BOOL gProximityHasValue = NO;
static BOOL gProximityNear = NO;
static BOOL gProximitySavedEnabled = NO;
static id gProximityToken = nil; // main でのみ触る（非 nil は「復元が未了」も表す）

static void BeginAltimeter(void);
static void InstallProximityObserver(void);

// 背景・前景の observer を 1 回だけ登録する。呼び出しは main から。
// 背景遷移: 気圧の購読を止め、近接の observer を外して有効化前の状態へ戻す。希望状態
// （gPressureActive / gProximityActive）は残し、前景復帰で希望されている購読だけを再開する。
static void RegisterLifecycleObservers(void) {
    if (gBackgroundToken != nil) {
        return;
    }
    gBackgroundToken = [NSNotificationCenter.defaultCenter
        addObserverForName:UIApplicationDidEnterBackgroundNotification
                    object:nil
                     queue:NSOperationQueue.mainQueue
                usingBlock:^(NSNotification *__unused note) {
                    os_unfair_lock_lock(&gLock);
                    gBackground = YES;
                    gPressureHasValue = NO;
                    gPressureFailure = 0; // 背景前の失敗を復帰後の購読へ持ち越さない
                    gProximityHasValue = NO;
                    CMAltimeter *old = gAltimeter;
                    gAltimeter = nil;
                    gPressureGen++; // 旧 handler の通知は世代照合で捨てられる
                    BOOL proximityActive = gProximityActive;
                    BOOL saved = gProximitySavedEnabled;
                    os_unfair_lock_unlock(&gLock);
                    [old stopRelativeAltitudeUpdates];
                    if (proximityActive) {
                        if (gProximityToken != nil) {
                            [NSNotificationCenter.defaultCenter removeObserver:gProximityToken];
                            gProximityToken = nil;
                        }
                        UIDevice.currentDevice.proximityMonitoringEnabled = saved;
                    }
                    fandhe_rust_environment_sensors_backgrounded();
                }];
    // proximityState を読むため main queue で受ける。
    gForegroundToken = [NSNotificationCenter.defaultCenter
        addObserverForName:UIApplicationWillEnterForegroundNotification
                    object:nil
                     queue:NSOperationQueue.mainQueue
                usingBlock:^(NSNotification *__unused note) {
                    os_unfair_lock_lock(&gLock);
                    gBackground = NO;
                    BOOL proximityActive = gProximityActive;
                    os_unfair_lock_unlock(&gLock);
                    if (proximityActive) {
                        UIDevice *device = UIDevice.currentDevice;
                        device.proximityMonitoringEnabled = YES;
                        BOOL enabled = device.proximityMonitoringEnabled;
                        BOOL near = device.proximityState;
                        if (enabled) {
                            InstallProximityObserver();
                        }
                        os_unfair_lock_lock(&gLock);
                        if (gProximityActive) {
                            gProximityNear = near;
                            gProximityHasValue = enabled;
                        }
                        os_unfair_lock_unlock(&gLock);
                    }
                    BeginAltimeter(); // 希望されている場合のみ再開する（内部で判定）
                    fandhe_rust_environment_sensors_resumed();
                }];
}

static int32_t PressureErrorCode(NSError *error) {
    if ([error.domain isEqualToString:CMErrorDomain] &&
        (error.code == CMErrorNotAuthorized || error.code == CMErrorMotionActivityNotAuthorized)) {
        return 2;
    }
    return 3;
}

// 気圧の購読を開始する。希望されていて（gPressureActive）背景でなく、未購読のときだけ動く。
static void BeginAltimeter(void) {
    os_unfair_lock_lock(&gLock);
    if (!gPressureActive || gBackground || gAltimeter != nil) {
        os_unfair_lock_unlock(&gLock);
        return;
    }
    if (gPressureQueue == nil) {
        gPressureQueue = [[NSOperationQueue alloc] init];
        gPressureQueue.maxConcurrentOperationCount = 1;
    }
    uint64_t gen = ++gPressureGen;
    CMAltimeter *alt = [[CMAltimeter alloc] init];
    gAltimeter = alt;
    gPressureHasValue = NO;
    gPressureFailure = 0; // 新しい世代は未失敗から始める（旧世代の失敗を残さない）
    gPressureSince = NSProcessInfo.processInfo.systemUptime;
    NSOperationQueue *queue = gPressureQueue;
    os_unfair_lock_unlock(&gLock);

    [alt startRelativeAltitudeUpdatesToQueue:queue
                                 withHandler:^(CMAltitudeData *data, NSError *error) {
        os_unfair_lock_lock(&gLock);
        if (!gPressureActive || gBackground || gen != gPressureGen) { // 旧購読・背景中は捨てる
            os_unfair_lock_unlock(&gLock);
            return;
        }
        if (error != nil) {
            gPressureHasValue = NO;
            gPressureFailure = PressureErrorCode(error);
        } else if (data == nil || data.pressure == nil || !isfinite(data.pressure.doubleValue)) {
            gPressureHasValue = NO;
        } else {
            double nowSec = NSProcessInfo.processInfo.systemUptime;
            double age = nowSec - data.timestamp;
            if (data.timestamp < gPressureSince || age < 0 || age > kPressureStaleSec) {
                gPressureHasValue = NO; // 開始前・未来・古い測定は捨てて次を待つ
            } else {
                gPressureKpa = data.pressure.doubleValue;
                uint64_t now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
                uint64_t ageNs = (uint64_t)(age * (double)NSEC_PER_SEC);
                gPressureStampNs = now > ageNs ? now - ageNs : 0;
                gPressureHasValue = YES;
                gPressureFailure = 0;
            }
        }
        os_unfair_lock_unlock(&gLock);
        fandhe_rust_environment_sensors_changed();
    }];
}

int32_t fandhe_ios_env_pressure_start(void) {
    @autoreleasepool {
        if (![CMAltimeter isRelativeAltitudeAvailable]) {
            return 1;
        }
        CMAuthorizationStatus auth = [CMAltimeter authorizationStatus];
        if (auth == CMAuthorizationStatusDenied || auth == CMAuthorizationStatusRestricted) {
            return 2;
        }
        if ([NSThread isMainThread]) {
            RegisterLifecycleObservers();
        }
        os_unfair_lock_lock(&gLock);
        if (gPressureActive) {
            os_unfair_lock_unlock(&gLock);
            return 0;
        }
        gPressureActive = YES;
        gPressureHasValue = NO;
        gPressureFailure = 0;
        os_unfair_lock_unlock(&gLock);
        BeginAltimeter(); // 背景中なら前景復帰で再開される
    }
    return 0;
}

int32_t fandhe_ios_env_pressure_read(double *kpa, double *age_ms) {
    if (kpa == NULL || age_ms == NULL) {
        return 3;
    }
    int32_t rc = 0;
    os_unfair_lock_lock(&gLock);
    if (gPressureActive) {
        if (gPressureFailure != 0) {
            rc = gPressureFailure;
        } else if (gPressureHasValue && !gBackground) {
            uint64_t now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
            if (now >= gPressureStampNs) {
                *kpa = gPressureKpa;
                *age_ms = (double)(now - gPressureStampNs) / 1.0e6;
                rc = 1;
            }
        }
    }
    os_unfair_lock_unlock(&gLock);
    return rc;
}

void fandhe_ios_env_pressure_stop(void) {
    os_unfair_lock_lock(&gLock);
    CMAltimeter *old = gAltimeter;
    gAltimeter = nil;
    gPressureGen++; // 旧 handler の通知はここから世代照合で捨てられる
    gPressureActive = NO;
    gPressureHasValue = NO;
    gPressureFailure = 0;
    os_unfair_lock_unlock(&gLock);
    [old stopRelativeAltitudeUpdates];
}

// main 限定。observer を外し、有効化前の状態へ戻す。世代が進んでいれば（再開済み）何もしない。
static void ProximityTeardown(uint64_t gen) {
    os_unfair_lock_lock(&gLock);
    if (gen != gProximityGen || gProximityActive) {
        os_unfair_lock_unlock(&gLock);
        return;
    }
    BOOL saved = gProximitySavedEnabled;
    os_unfair_lock_unlock(&gLock);
    if (gProximityToken != nil) {
        [NSNotificationCenter.defaultCenter removeObserver:gProximityToken];
        gProximityToken = nil;
    }
    UIDevice.currentDevice.proximityMonitoringEnabled = saved;
}

// main 限定。近接の変化 observer を張り直す（既存があれば外してから）。
static void InstallProximityObserver(void) {
    if (gProximityToken != nil) {
        [NSNotificationCenter.defaultCenter removeObserver:gProximityToken];
        gProximityToken = nil;
    }
    gProximityToken = [NSNotificationCenter.defaultCenter
        addObserverForName:UIDeviceProximityStateDidChangeNotification
                    object:nil
                     queue:NSOperationQueue.mainQueue
                usingBlock:^(NSNotification *__unused note) {
                    BOOL near = UIDevice.currentDevice.proximityState;
                    os_unfair_lock_lock(&gLock);
                    BOOL accept = gProximityActive && !gBackground;
                    if (accept) {
                        gProximityNear = near;
                        gProximityHasValue = YES;
                    }
                    os_unfair_lock_unlock(&gLock);
                    if (accept) {
                        fandhe_rust_environment_sensors_changed();
                    }
                }];
}

int32_t fandhe_ios_env_proximity_start(void) {
    if (![NSThread isMainThread]) {
        return 3;
    }
    @autoreleasepool {
        RegisterLifecycleObservers();
        os_unfair_lock_lock(&gLock);
        if (gProximityActive) {
            os_unfair_lock_unlock(&gLock);
            return 0;
        }
        os_unfair_lock_unlock(&gLock);

        UIDevice *device = UIDevice.currentDevice;
        // 停止直後の復元が main に残っている（トークンが非 nil）場合、現在値は自分が有効にした値なので
        // 保存値を上書きしない。
        BOOL pendingRestore = gProximityToken != nil;
        BOOL saved = pendingRestore ? gProximitySavedEnabled : device.proximityMonitoringEnabled;
        device.proximityMonitoringEnabled = YES;
        if (!device.proximityMonitoringEnabled) { // 非搭載は NO のまま
            device.proximityMonitoringEnabled = saved;
            return 1;
        }
        InstallProximityObserver();
        BOOL near = device.proximityState;
        os_unfair_lock_lock(&gLock);
        gProximityGen++;
        gProximitySavedEnabled = saved;
        gProximityActive = YES;
        gProximityNear = near;
        gProximityHasValue = !gBackground; // on-change は変化まで来ないので初期値を持つ
        os_unfair_lock_unlock(&gLock);
    }
    return 0;
}

int32_t fandhe_ios_env_proximity_read(int32_t *near) {
    if (near == NULL) {
        return 3;
    }
    int32_t rc = 0;
    os_unfair_lock_lock(&gLock);
    if (gProximityActive && gProximityHasValue && !gBackground) {
        *near = gProximityNear ? 1 : 0;
        rc = 1;
    }
    os_unfair_lock_unlock(&gLock);
    return rc;
}

void fandhe_ios_env_proximity_stop(void) {
    os_unfair_lock_lock(&gLock);
    if (!gProximityActive) {
        os_unfair_lock_unlock(&gLock);
        return;
    }
    gProximityActive = NO;
    gProximityHasValue = NO;
    uint64_t gen = ++gProximityGen;
    os_unfair_lock_unlock(&gLock);
    if ([NSThread isMainThread]) {
        ProximityTeardown(gen);
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{
            ProximityTeardown(gen);
        });
    }
}
