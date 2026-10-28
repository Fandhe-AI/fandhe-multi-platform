// AR 表示: iOS 実装（Issue #308、親 #90 の (b-ios)）。ARKit の `ARSession`（ARWorldTrackingConfiguration、
// 水平面検出）から追跡状態・開始地点からの位置・水平面の数だけを写し、static に保持する（pull 型）。
// カメラ映像（Issue #1009、(b-ios)）は `fandhe_ios_ar_read_frame` が `currentFrame.capturedImage` の生の面を
// Rust へ同期に渡し、RGB 変換と縮小は Rust が行う。`ARFrame` / pixel buffer は呼び出しの中で手放し保持しない。
//
// `src/device/ios_ar.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の feature `device` 有効時のみ
// `cc` コンパイルし ARKit をリンクする。
//
// 契約:
//   fandhe_ios_ar_start() -> int32_t   0 = 成功（冪等）、1 = 非対応（`isSupported` が NO。シミュレータはここ）、
//                                      2 = 権限拒否、3 = 失敗（main 以外から呼ばれた場合を含む）
//   fandhe_ios_ar_read(int32_t *tracking, double *pos /* [3] */, int32_t *planes) -> int32_t
//                                      0 = 値なし、1 = 値あり（out へ書く）、2 = 権限拒否、3 = 失敗、
//                                      4 = 許可待ち、5 = 非対応
//     tracking: 0 NotAvailable / 1 Normal / 10 Initializing / 11 ExcessiveMotion /
//               12 InsufficientFeatures / 13 Relocalizing / 19 Other
//   fandhe_ios_ar_read_frame(void *ctx)  main 限定。最新の `capturedImage`（二面 YCbCr 4:2:0 full range）を
//                                      lock して `fandhe_rust_ar_frame(ctx, ...)` へ同期に 0〜1 回渡す。
//                                      非 main・停止・背景・nil・形式違い・寸法不整合は何も呼ばない（映像なし）
//   fandhe_ios_ar_stop()               冪等。保持値と失敗を捨て、セッションを止める
//   状態が変わった直後に Rust の `fandhe_rust_ar_changed()` を呼ぶ（再描画要求）。背景・前景遷移は
//   `fandhe_rust_ar_backgrounded` / `fandhe_rust_ar_resumed` で Rust に知らせる。
//
// カメラの許可が未決定なら `requestAccessForMediaType` で要求し、応答まで session を動かさない
// （その間 read は 4 を返す）。権限ダイアログ中の Inactive は背景として扱わない。
// 保持値は受信から 1 秒を超えたら返さない（単調時計。途絶後に古い値を返し続けない）。
// 背景遷移で値を捨てる。中断（sessionWasInterrupted）で値を捨て、中断終了で原点と水平面を取り直す。
// 位置は毎フレーム変わるので再描画要求は出さず、driver の 100 ms pull で拾う。追跡状態・
// 水平面の数・値の有無が変わったときだけ再描画を要求する（契約表 4）。
//
// スレッド: session の生成・run は main 限定。delegate は専用 serial queue で動く。状態は
// os_unfair_lock で守り、lock の中で Rust のコールバックや重い ObjC 呼び出しをしない。
// stop は main 以外なら旧 session の pause を dispatch_async で main へ送る（dispatch_sync 不使用）。
//
// 守る範囲: 嘘の値を出さない・二重に開始しない・停止と失敗で session を止める・古い世代の通知を捨てる。
// 守らない範囲: 姿勢・画面回転・hit-test・アンカー、OS 間の値の一致、複数プロセス、表示の遅れ
// （最大 100 ms + フレーム周期）。
// ログは出さない（位置・追跡状態・NSError を出さない。security.md A09）。

#import <ARKit/ARKit.h>
#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <CoreVideo/CoreVideo.h>
#import <os/lock.h>
#include <time.h>

extern void fandhe_rust_ar_changed(void);
extern void fandhe_rust_ar_backgrounded(void);
extern void fandhe_rust_ar_resumed(void);
extern void fandhe_rust_ar_frame(void *ctx, uint32_t width, uint32_t height, const uint8_t *y,
                                 size_t y_len, size_t y_stride, const uint8_t *uv, size_t uv_len,
                                 size_t uv_stride);

static os_unfair_lock gLock = OS_UNFAIR_LOCK_INIT;
static BOOL gActive = NO;
static BOOL gHasValue = NO;
static BOOL gBackground = NO;
static BOOL gAwaitingPermission = NO;
static int32_t gFailure = 0; // 0 = なし、2 = 権限拒否、3 = 失敗、5 = 非対応
static uint64_t gGeneration = 0;
static int32_t gTracking = 0;
static double gPos[3] = {0, 0, 0};
static int32_t gPlanes = 0;
static uint64_t gStampNs = 0;
static const uint64_t kStaleNs = 1ULL * NSEC_PER_SEC;

@interface FandheIosArDelegate : NSObject <ARSessionDelegate>
@end

static ARSession *gSession = nil;
static FandheIosArDelegate *gDelegate = nil;
static dispatch_queue_t gQueue = nil;
static id gBackgroundToken = nil;
static id gForegroundToken = nil;

static ARWorldTrackingConfiguration *MakeConfig(void) {
    ARWorldTrackingConfiguration *c = [[ARWorldTrackingConfiguration alloc] init];
    c.planeDetection = ARPlaneDetectionHorizontal;
    return c;
}

static const ARSessionRunOptions kRunOptions =
    ARSessionRunOptionResetTracking | ARSessionRunOptionRemoveExistingAnchors;

static int32_t TrackingCode(ARCamera *camera) {
    switch (camera.trackingState) {
    case ARTrackingStateNotAvailable:
        return 0;
    case ARTrackingStateNormal:
        return 1;
    case ARTrackingStateLimited:
        switch (camera.trackingStateReason) {
        case ARTrackingStateReasonInitializing:
            return 10;
        case ARTrackingStateReasonExcessiveMotion:
            return 11;
        case ARTrackingStateReasonInsufficientFeatures:
            return 12;
        case ARTrackingStateReasonRelocalizing:
            return 13;
        default:
            return 19;
        }
    }
    return 19;
}

@implementation FandheIosArDelegate

- (void)session:(ARSession *)session didUpdateFrame:(ARFrame *)frame {
    @autoreleasepool {
        // frame から数値だけを写す。frame は保持しない（カメラのバッファが枯渇する）。
        int32_t tracking = TrackingCode(frame.camera);
        simd_float4 t = frame.camera.transform.columns[3];
        int32_t planes = 0;
        for (ARAnchor *a in frame.anchors) {
            if ([a isKindOfClass:[ARPlaneAnchor class]] &&
                ((ARPlaneAnchor *)a).alignment == ARPlaneAnchorAlignmentHorizontal) {
                planes++;
            }
        }
        os_unfair_lock_lock(&gLock);
        if (!gActive || gBackground || session != gSession) {
            os_unfair_lock_unlock(&gLock);
            return;
        }
        BOOL changed = !gHasValue || tracking != gTracking || planes != gPlanes;
        gTracking = tracking;
        gPos[0] = t.x;
        gPos[1] = t.y;
        gPos[2] = t.z;
        gPlanes = planes;
        gStampNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
        gHasValue = YES;
        gFailure = 0;
        os_unfair_lock_unlock(&gLock);
        if (changed) {
            fandhe_rust_ar_changed();
        }
    }
}

- (void)session:(ARSession *)session didFailWithError:(NSError *)error {
    int32_t code = 3;
    if ([error.domain isEqualToString:ARErrorDomain]) {
        if (error.code == ARErrorCodeUnsupportedConfiguration) {
            code = 5;
        } else if (error.code == ARErrorCodeCameraUnauthorized) {
            code = 2;
        }
    }
    os_unfair_lock_lock(&gLock);
    if (!gActive || session != gSession) {
        os_unfair_lock_unlock(&gLock);
        return;
    }
    gFailure = code;
    gHasValue = NO;
    os_unfair_lock_unlock(&gLock);
    // pause と解放は、driver が Err を受けて呼ぶ stop() で行う。
    fandhe_rust_ar_changed();
}

- (void)sessionWasInterrupted:(ARSession *)session {
    os_unfair_lock_lock(&gLock);
    if (gActive && session == gSession) {
        gHasValue = NO;
    }
    os_unfair_lock_unlock(&gLock);
}

- (void)sessionInterruptionEnded:(ARSession *)session {
    os_unfair_lock_lock(&gLock);
    uint64_t gen = gGeneration;
    os_unfair_lock_unlock(&gLock);
    dispatch_async(dispatch_get_main_queue(), ^{
        os_unfair_lock_lock(&gLock);
        BOOL ok = gActive && gen == gGeneration && session == gSession;
        if (ok) {
            gHasValue = NO;
        }
        os_unfair_lock_unlock(&gLock);
        if (ok) {
            // 原点と水平面を取り直す。
            [session runWithConfiguration:MakeConfig() options:kRunOptions];
            fandhe_rust_ar_changed();
        }
    });
}

- (BOOL)sessionShouldAttemptRelocalization:(ARSession *)session {
    return NO;
}

@end

// main で呼ぶ。gActive / 世代が一致するときだけ session を保持して run する。
static void RunNewSession(uint64_t gen) {
    ARSession *s = [[ARSession alloc] init];
    FandheIosArDelegate *d = [[FandheIosArDelegate alloc] init];
    if (gQueue == nil) {
        gQueue = dispatch_queue_create("fandhe.ios.ar.delegate", DISPATCH_QUEUE_SERIAL);
    }
    s.delegate = d;
    s.delegateQueue = gQueue;
    os_unfair_lock_lock(&gLock);
    if (!gActive || gen != gGeneration) {
        os_unfair_lock_unlock(&gLock);
        return;
    }
    gSession = s;
    gDelegate = d;
    gAwaitingPermission = NO;
    os_unfair_lock_unlock(&gLock);
    [s runWithConfiguration:MakeConfig() options:kRunOptions];
}

int32_t fandhe_ios_ar_start(void) {
    if (![NSThread isMainThread]) {
        return 3;
    }
    @autoreleasepool {
        if (![ARWorldTrackingConfiguration isSupported]) {
            return 1;
        }
        AVAuthorizationStatus auth = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo];
        if (auth == AVAuthorizationStatusDenied || auth == AVAuthorizationStatusRestricted) {
            return 2;
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
                            fandhe_rust_ar_backgrounded();
                        }];
            gForegroundToken = [NSNotificationCenter.defaultCenter
                addObserverForName:UIApplicationWillEnterForegroundNotification
                            object:nil
                             queue:nil
                        usingBlock:^(NSNotification *__unused note) {
                            os_unfair_lock_lock(&gLock);
                            gBackground = NO;
                            gHasValue = NO;
                            os_unfair_lock_unlock(&gLock);
                            fandhe_rust_ar_resumed();
                        }];
        }
        os_unfair_lock_lock(&gLock);
        if (gActive) {
            os_unfair_lock_unlock(&gLock);
            return 0;
        }
        gGeneration++;
        uint64_t gen = gGeneration;
        gActive = YES;
        gHasValue = NO;
        gFailure = 0;
        gAwaitingPermission = (auth == AVAuthorizationStatusNotDetermined);
        gBackground = UIApplication.sharedApplication.applicationState == UIApplicationStateBackground;
        os_unfair_lock_unlock(&gLock);
        if (auth == AVAuthorizationStatusNotDetermined) {
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo
                                     completionHandler:^(BOOL granted) {
                                         // 任意スレッドで届くので main へ移り、世代で照合する。
                                         dispatch_async(dispatch_get_main_queue(), ^{
                                             if (granted) {
                                                 RunNewSession(gen);
                                             } else {
                                                 os_unfair_lock_lock(&gLock);
                                                 if (gActive && gen == gGeneration) {
                                                     gAwaitingPermission = NO;
                                                     gFailure = 2;
                                                 }
                                                 os_unfair_lock_unlock(&gLock);
                                             }
                                             fandhe_rust_ar_changed();
                                         });
                                     }];
        } else {
            RunNewSession(gen);
        }
    }
    return 0;
}

int32_t fandhe_ios_ar_read(int32_t *tracking, double *pos, int32_t *planes) {
    if (tracking == NULL || pos == NULL || planes == NULL) {
        return 3;
    }
    int32_t rc = 0;
    os_unfair_lock_lock(&gLock);
    if (gActive) {
        if (gFailure != 0) {
            rc = gFailure;
        } else if (gAwaitingPermission) {
            rc = 4;
        } else if (gHasValue && clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - gStampNs <= kStaleNs) {
            *tracking = gTracking;
            pos[0] = gPos[0];
            pos[1] = gPos[1];
            pos[2] = gPos[2];
            *planes = gPlanes;
            rc = 1;
        }
    }
    os_unfair_lock_unlock(&gLock);
    return rc;
}

void fandhe_ios_ar_stop(void) {
    os_unfair_lock_lock(&gLock);
    ARSession *old = gSession;
    gSession = nil;
    gDelegate = nil;
    gGeneration++;
    gActive = NO;
    gHasValue = NO;
    gFailure = 0;
    gAwaitingPermission = NO;
    os_unfair_lock_unlock(&gLock);
    if (old == nil) {
        return;
    }
    if ([NSThread isMainThread]) {
        [old pause];
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{
            [old pause];
        });
    }
}

void fandhe_ios_ar_read_frame(void *ctx) {
    if (ctx == NULL || !NSThread.isMainThread) {
        return;
    }
    @autoreleasepool {
        os_unfair_lock_lock(&gLock);
        ARSession *s = (gActive && !gBackground && gHasValue) ? gSession : nil;
        os_unfair_lock_unlock(&gLock);
        // ARFrame を変換完了まで強参照し、session 側の差し替えで pixel buffer が解放されるのを防ぐ
        ARFrame *__attribute__((objc_precise_lifetime)) frame = s.currentFrame;
        CVPixelBufferRef pb = frame.capturedImage;
        if (pb == NULL ||
            CVPixelBufferGetPixelFormatType(pb) != kCVPixelFormatType_420YpCbCr8BiPlanarFullRange ||
            !CVPixelBufferIsPlanar(pb) || CVPixelBufferGetPlaneCount(pb) != 2) {
            return;
        }
        if (CVPixelBufferLockBaseAddress(pb, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
            return;
        }
        size_t w = CVPixelBufferGetWidthOfPlane(pb, 0);
        size_t h = CVPixelBufferGetHeightOfPlane(pb, 0);
        size_t cw = CVPixelBufferGetWidthOfPlane(pb, 1);
        size_t ch = CVPixelBufferGetHeightOfPlane(pb, 1);
        const uint8_t *y = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
        const uint8_t *uv = CVPixelBufferGetBaseAddressOfPlane(pb, 1);
        size_t ys = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
        size_t uvs = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
        if (w > 0 && h > 0 && w <= UINT32_MAX && h <= UINT32_MAX && y != NULL && uv != NULL &&
            cw == (w + 1) / 2 && ch == (h + 1) / 2) {
            fandhe_rust_ar_frame(ctx, (uint32_t)w, (uint32_t)h, y, ys * h, ys, uv, uvs * ch, uvs);
        }
        CVPixelBufferUnlockBaseAddress(pb, kCVPixelBufferLock_ReadOnly);
    }
}
