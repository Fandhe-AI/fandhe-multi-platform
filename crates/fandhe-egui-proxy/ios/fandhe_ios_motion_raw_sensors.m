// ジャイロ・磁気・線形加速度の生値: iOS 実装（Issue #900、親 #180）。
// CoreMotion `CMMotionManager` の `gyroData.rotationRate`（rad/s）・`magnetometerData.magneticField`
// （µT）・`deviceMotion.userAcceleration`（G）を OS が返した単位と軸のまま保持する（pull 型）。
//
// `src/device/ios_motion_raw_sensors.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（CoreMotion・UIKit は他の .m 用にリンク済み）。
//
// manager は `fandhe_ios_motion.m`（tilt）と共有する 1 つだけを使う。そちらを正とし、ここでは
// extern 宣言のみ置く。deviceMotion は tilt が動かしているので、こちらからは止めない
// （止めるのはジャイロと磁気だけ）。
//
// 契約（kind: 0 = 角速度、1 = 磁場、2 = 線形加速度。範囲外は失敗）:
//   fandhe_ios_motion_raw_start(kind) -> int32_t   0 = 成功（冪等）、1 = 端末に無い（is*Available == NO）、
//                                                  3 = 失敗（main 以外から呼んだ場合・不正な kind）
//   fandhe_ios_motion_raw_read(kind, *x, *y, *z) -> int32_t
//                                                  0 = 値なし、1 = 値あり（out へ書く）、2 = 権限拒否、3 = 失敗
//   fandhe_ios_motion_raw_stop(kind)               冪等。どのスレッドからでも呼べる
//
// 古い値の判定は OS が示す事実だけで行う: サンプルの `timestamp`（起動からの秒、単調）が
// 開始時・前景復帰時の `NSProcessInfo.systemUptime` より前なら値なし。期限・古さの閾値は置かない。
// 背景遷移ではジャイロと磁気の購読を止めて値を捨て、前景復帰で利用者が開始したままの分だけ張り直す。
// センサー値・エラーはログへ出さず、プロセス内の static に置くだけ（security.md A01）。

#import <CoreMotion/CoreMotion.h>
#import <UIKit/UIKit.h>
#import <os/lock.h>

// `fandhe_ios_motion.m` が定義する共有 manager と device motion の起動（tilt と共有）。
extern CMMotionManager *fandhe_ios_shared_motion_manager(void);
extern int32_t fandhe_ios_device_motion_ensure_started(void);

// Rust 側（`ios_motion_raw_sensors.rs`）の受け口。値・失敗を確定した直後に次フレームを起こす。
extern void fandhe_rust_motion_raw_sensors_changed(void);

enum { kKindGyro = 0, kKindMag = 1, kKindLinear = 2, kKindCount = 3 };

typedef struct {
    BOOL active;      // 利用者が開始したままか（希望状態）
    BOOL hasValue;
    double x, y, z;
    int32_t failure;  // 0 / 2（権限拒否）/ 3（失敗）
    NSTimeInterval since;  // この時刻（systemUptime）より前のサンプルは捨てる
    uint64_t gen;
} FandheRawState;

static os_unfair_lock gLock = OS_UNFAIR_LOCK_INIT;
static FandheRawState gState[kKindCount];
static BOOL gBackground = NO;
static BOOL gObserving = NO;
static NSOperationQueue *gQueue = nil;  // ジャイロ・磁気の handler 用シリアルキュー

// CoreMotion の権限拒否 2 種は 2、他は 3（元のコードを 1 つに潰さない）。
static int32_t FandheFailureForError(NSError *error) {
    if ([error.domain isEqualToString:CMErrorDomain] &&
        (error.code == CMErrorNotAuthorized || error.code == CMErrorMotionActivityNotAuthorized)) {
        return 2;
    }
    return 3;
}

// ジャイロ・磁気の購読を開始する（ロックの外で呼ぶ）。`gen` は開始時点の世代。
static void FandheStartUpdates(int32_t kind, uint64_t gen) {
    CMMotionManager *manager = fandhe_ios_shared_motion_manager();
    void (^store)(BOOL, double, double, double, NSTimeInterval, NSError *) =
        ^(BOOL hasData, double x, double y, double z, NSTimeInterval ts, NSError *error) {
          os_unfair_lock_lock(&gLock);
          FandheRawState *s = &gState[kind];
          BOOL current = s->active && !gBackground && s->gen == gen;
          if (current) {
              if (error != nil) {
                  s->failure = FandheFailureForError(error);
                  s->hasValue = NO;
              } else if (hasData && ts >= s->since) {
                  s->failure = 0;
                  s->hasValue = YES;
                  s->x = x;
                  s->y = y;
                  s->z = z;
              } else {
                  current = NO;
              }
          }
          os_unfair_lock_unlock(&gLock);
          if (current) {
              fandhe_rust_motion_raw_sensors_changed();
          }
        };
    if (kind == kKindGyro) {
        manager.gyroUpdateInterval = 0.1;
        [manager startGyroUpdatesToQueue:gQueue
                             withHandler:^(CMGyroData *d, NSError *e) {
                               store(d != nil, d.rotationRate.x, d.rotationRate.y,
                                     d.rotationRate.z, d.timestamp, e);
                             }];
    } else {
        manager.magnetometerUpdateInterval = 0.1;
        [manager startMagnetometerUpdatesToQueue:gQueue
                                     withHandler:^(CMMagnetometerData *d, NSError *e) {
                                       store(d != nil, d.magneticField.x, d.magneticField.y,
                                             d.magneticField.z, d.timestamp, e);
                                     }];
    }
}

static void FandheStopUpdates(int32_t kind) {
    CMMotionManager *manager = fandhe_ios_shared_motion_manager();
    if (kind == kKindGyro) {
        [manager stopGyroUpdates];
    } else if (kind == kKindMag) {
        [manager stopMagnetometerUpdates];
    }
}

// 背景・前景の observer を 1 回だけ登録する（main から呼ぶ）。
static void FandheObserveLifecycleOnce(void) {
    if (gObserving) {
        return;
    }
    gObserving = YES;
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    [center addObserverForName:UIApplicationDidEnterBackgroundNotification
                        object:nil
                         queue:NSOperationQueue.mainQueue
                    usingBlock:^(NSNotification *note) {
                      (void)note;
                      os_unfair_lock_lock(&gLock);
                      gBackground = YES;
                      for (int k = 0; k < kKindCount; k++) {
                          gState[k].hasValue = NO;
                          gState[k].failure = 0;
                      }
                      gState[kKindGyro].gen++;
                      gState[kKindMag].gen++;
                      os_unfair_lock_unlock(&gLock);
                      FandheStopUpdates(kKindGyro);
                      FandheStopUpdates(kKindMag);
                    }];
    [center addObserverForName:UIApplicationWillEnterForegroundNotification
                        object:nil
                         queue:NSOperationQueue.mainQueue
                    usingBlock:^(NSNotification *note) {
                      (void)note;
                      uint64_t gens[kKindCount] = {0};
                      BOOL restart[kKindCount] = {NO};
                      os_unfair_lock_lock(&gLock);
                      gBackground = NO;
                      NSTimeInterval now = NSProcessInfo.processInfo.systemUptime;
                      for (int k = 0; k < kKindCount; k++) {
                          gState[k].since = now;
                          gens[k] = gState[k].gen;
                          restart[k] = gState[k].active && k != kKindLinear;
                      }
                      os_unfair_lock_unlock(&gLock);
                      for (int k = 0; k < kKindCount; k++) {
                          if (restart[k]) {
                              FandheStartUpdates(k, gens[k]);
                          }
                      }
                      fandhe_rust_motion_raw_sensors_changed();
                    }];
}

int32_t fandhe_ios_motion_raw_start(int32_t kind) {
    if (kind < 0 || kind >= kKindCount || ![NSThread isMainThread]) {
        return 3;
    }
    @autoreleasepool {
        CMMotionManager *manager = fandhe_ios_shared_motion_manager();
        BOOL available = kind == kKindGyro   ? manager.isGyroAvailable
                         : kind == kKindMag ? manager.isMagnetometerAvailable
                                            : manager.isDeviceMotionAvailable;
        if (!available) {
            return 1;
        }
        static dispatch_once_t once;
        dispatch_once(&once, ^{
          gQueue = [[NSOperationQueue alloc] init];
          gQueue.maxConcurrentOperationCount = 1;
        });
        FandheObserveLifecycleOnce();

        os_unfair_lock_lock(&gLock);
        FandheRawState *s = &gState[kind];
        if (s->active) {
            os_unfair_lock_unlock(&gLock);
            return 0;
        }
        s->active = YES;
        s->hasValue = NO;
        s->failure = 0;
        s->since = NSProcessInfo.processInfo.systemUptime;
        uint64_t gen = ++s->gen;
        gBackground = UIApplication.sharedApplication.applicationState == UIApplicationStateBackground;
        BOOL background = gBackground;
        os_unfair_lock_unlock(&gLock);

        if (kind == kKindLinear) {
            if (fandhe_ios_device_motion_ensure_started() != 0) {
                os_unfair_lock_lock(&gLock);
                s->active = NO;
                s->gen++;
                os_unfair_lock_unlock(&gLock);
                return 1;
            }
        } else if (!background) {
            FandheStartUpdates(kind, gen);
        }
        return 0;
    }
}

int32_t fandhe_ios_motion_raw_read(int32_t kind, double *x, double *y, double *z) {
    if (x == NULL || y == NULL || z == NULL || kind < 0 || kind >= kKindCount) {
        return 3;
    }
    os_unfair_lock_lock(&gLock);
    FandheRawState s = gState[kind];
    BOOL live = s.active && !gBackground;
    os_unfair_lock_unlock(&gLock);
    if (!live) {
        return 0;
    }
    if (kind != kKindLinear) {
        if (s.failure != 0) {
            return s.failure;
        }
        if (!s.hasValue) {
            return 0;
        }
        *x = s.x;
        *y = s.y;
        *z = s.z;
        return 1;
    }
    @autoreleasepool {
        CMDeviceMotion *motion = fandhe_ios_shared_motion_manager().deviceMotion;
        if (motion == nil || motion.timestamp < s.since) {
            return 0;
        }
        *x = motion.userAcceleration.x;
        *y = motion.userAcceleration.y;
        *z = motion.userAcceleration.z;
        return 1;
    }
}

void fandhe_ios_motion_raw_stop(int32_t kind) {
    if (kind < 0 || kind >= kKindCount) {
        return;
    }
    os_unfair_lock_lock(&gLock);
    FandheRawState *s = &gState[kind];
    s->active = NO;
    s->hasValue = NO;
    s->failure = 0;
    s->gen++;
    os_unfair_lock_unlock(&gLock);
    // 線形加速度の deviceMotion は tilt と共有のため止めない。
    if (kind != kKindLinear) {
        @autoreleasepool {
            FandheStopUpdates(kind);
        }
    }
}
