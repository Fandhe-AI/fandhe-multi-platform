// Phase 11 の拡充ループ（親 #97）型サンプル「ピクチャ・イン・ピクチャ」: macOS（Issue #364）の実装。
// iOS（b-ios）と共用する前提で apple/ に置く。macOS 固有の部分は TARGET_OS_OSX で囲む。
// 実行時に生成した単色フレーム（RGB の表を interval_ms ごとに切り替え）を AVSampleBufferDisplayLayer へ
// 流し、AVKit の sample buffer 経路（AVPictureInPictureControllerContentSource、macOS 12.0+）で OS の
// 小窓へ出す。素材ファイル・URL・ネットワークは使わない。
//
// `crates/fandhe-egui-proxy/src/device/apple_picture_in_picture.rs` から C ABI 経由で呼ばれる。
// `build.rs` の macOS 分岐がコンパイルし AVKit・CoreMedia・CoreVideo・QuartzCore をリンクする。
// 12.0+ の API は @available で守る（実行体の最低版は 11.0。12 未満は prepare が非対応 = 1 を返す）。
//
// 契約（戻りコードは固定。NSError の生値・説明は返さずログにも出さない、security.md A09）:
//   0 = 成功、1 = 非対応（macOS 12 未満・isPictureInPictureSupported が NO）、2 = 引数不正、
//   3 = 内部（窓・バッファ・controller を作れない）、4 = main thread 以外から呼ばれた、
//   5 = いま小窓にできない、6 = 世代不一致（旧世代・未準備を含む）。
//   int32_t fandhe_apple_pip_prepare(uint64_t gen, const uint8_t *rgb, size_t color_count,
//                                    uint32_t interval_ms, uint32_t width, uint32_t height)
//     gen は Rust が採番する 0 以外の世代。rgb は色数 * 3 バイト（RGB）で、同期でコピーしポインタを保持しない。
//     前の資源を先に解放する。失敗しても作りかけを解放した状態で返す。
//   int32_t fandhe_apple_pip_start(uint64_t gen)  0 / 4 / 5 / 6。結果は snapshot で確定する。
//   int32_t fandhe_apple_pip_stop(uint64_t gen)   0 / 4 / 6。Starting 中は開始の完了後に止める。
//     Starting / Active 以外では何もせず 0 を返す。
//   void fandhe_apple_pip_snapshot(uint64_t *gen, int32_t *phase)
//     phase: 0 = Idle、1 = Preparing、2 = Ready、3 = Starting、4 = Active、5 = Stopping、6 = Exited、
//     7 = 開始失敗（NotPossible）、8 = 内部失敗。Preparing のとき isPictureInPicturePossible が YES なら
//     Ready にする（pull 型。KVO は使わない）。
//   void fandhe_apple_pip_shutdown(void)  冪等。
//
// 並行性（device-bridge-contracts.md 11）: 状態・世代は 1 本の g_lock の内側で更新する。
// AVKit の呼び出し（start / stop / 解放）は g_lock を外して行う（delegate が同期で呼ばれても
// デッドロックしない）。delegate は自分の世代を持ち、ロックの内側で g_gen と照合して古い世代の通知を捨てる。
// 呼び出しはすべて egui の update（macOS は main thread）から来る。main への dispatch_sync は使わない
// （呼び出し元が main のためデッドロックする）。
// 権限・entitlement・usage description は不要。映像の内容はログへ出力しない。

#import <TargetConditionals.h>
#if TARGET_OS_OSX

#import <AVFoundation/AVFoundation.h>
#import <AVKit/AVKit.h>
#import <AppKit/AppKit.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <QuartzCore/QuartzCore.h>
#include <pthread.h>
#include <stdint.h>

// AVSampleBufferDisplayLayer の enqueue 系は macOS 15 で sampleBufferRenderer 経由へ移行したが、
// 実行体の最低版は 11.0 のため旧 API を使う（15 未満では新 API が無い）。
#pragma clang diagnostic ignored "-Wdeprecated-declarations"

enum {
    PHASE_IDLE = 0,
    PHASE_PREPARING,
    PHASE_READY,
    PHASE_STARTING,
    PHASE_ACTIVE,
    PHASE_STOPPING,
    PHASE_EXITED,
    PHASE_FAILED_START,
    PHASE_FAILED_INTERNAL
};

enum {
    RC_OK = 0,
    RC_UNSUPPORTED = 1,
    RC_INVALID = 2,
    RC_INTERNAL = 3,
    RC_NOT_MAIN = 4,
    RC_NOT_POSSIBLE = 5,
    RC_STALE = 6
};

#define MAX_COLORS 16
#define MAX_DIMENSION 4096
// 画面右下に置くネイティブ映像の大きさ（pt）と余白。
#define PREVIEW_W 160.0
#define PREVIEW_H 90.0
#define PREVIEW_MARGIN 8.0

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static uint64_t g_gen = 0;
static int32_t g_phase = PHASE_IDLE;
// Starting 中に stop が呼ばれた。開始の完了後に必ず止める。
static BOOL g_stop_after_start = NO;
static AVSampleBufferDisplayLayer *g_layer = nil;
static AVPictureInPictureController *g_controller = nil;
static id g_delegate = nil;
// release_all で外した delegate（Starting / Active 中だったもの）。controller.delegate は weak のため、
// 開始・停止の完了通知が届くまで強参照で延命する。各 delegate が controller を保持し、
// 完了通知（停止・失敗）で配列から外す。複数世代が重なっても全部を追跡する。
static NSMutableArray *g_retired = nil;
static dispatch_source_t g_timer = nil;
static CVPixelBufferRef g_buffers[MAX_COLORS];
static size_t g_buffer_count = 0;
static CMVideoFormatDescriptionRef g_format = NULL;
static uint32_t g_interval_ms = 0;
static uint64_t g_tick = 0;

static void release_buffers_locked(void) {
    for (size_t i = 0; i < g_buffer_count; i++) {
        if (g_buffers[i]) {
            CVPixelBufferRelease(g_buffers[i]);
            g_buffers[i] = NULL;
        }
    }
    g_buffer_count = 0;
    if (g_format) {
        CFRelease(g_format);
        g_format = NULL;
    }
}

API_AVAILABLE(macos(12.0))
@interface FandhePipDelegate : NSObject <AVPictureInPictureControllerDelegate,
                                          AVPictureInPictureSampleBufferPlaybackDelegate>
@property(nonatomic) uint64_t gen;
// release_all 後（main thread で設定）。完了通知は世代照合ではなくこの印で扱う。
@property(nonatomic) BOOL retired;
@property(nonatomic, strong) AVPictureInPictureController *controller;
@end

// 契約表 9: タイマー cancel → stopPictureInPicture（delegate は停止完了まで生かす）→ controller nil →
// layer を flushAndRemoveImage して removeFromSuperlayer → 世代 0。main thread で呼ぶ。
static void release_all(void) {
    pthread_mutex_lock(&g_lock);
    dispatch_source_t timer = g_timer;
    AVPictureInPictureController *controller = g_controller;
    AVSampleBufferDisplayLayer *layer = g_layer;
    g_timer = nil;
    g_controller = nil;
    g_layer = nil;
    id delegate = g_delegate;
    BOOL was_starting = (g_phase == PHASE_STARTING);
    g_delegate = nil;
    g_gen = 0;
    g_phase = PHASE_IDLE;
    g_stop_after_start = NO;
    release_buffers_locked();
    pthread_mutex_unlock(&g_lock);

    if (timer) dispatch_source_cancel(timer);
    // g_gen = 0 のため、旧 delegate の通常通知は世代照合で捨てられる。Starting 中なら retired の
    // delegate が遅れて届く DidStart で停止し、小窓を残さない。
    if (controller) {
        if (controller.isPictureInPictureActive || was_starting) {
            FandhePipDelegate *d = delegate;
            d.retired = YES;
            d.controller = controller;
            if (!g_retired) g_retired = [NSMutableArray array];
            [g_retired addObject:d];
            if (controller.isPictureInPictureActive) [controller stopPictureInPicture];
        } else {
            controller.delegate = nil;
        }
    }
    if (layer) {
        [layer flushAndRemoveImage];
        [layer removeFromSuperlayer];
    }
}

@implementation FandhePipDelegate

- (void)pictureInPictureControllerDidStartPictureInPicture:(AVPictureInPictureController *)c {
    BOOL stop_now = NO;
    pthread_mutex_lock(&g_lock);
    if (self.retired) {
        // 解放済みの世代が遅れて開始した。止めて、停止通知で外す。
        stop_now = YES;
    } else if (self.gen == g_gen) {
        if (g_stop_after_start) {
            // 開始中のキャンセル。開始の完了後に止める（契約表 3）。
            g_phase = PHASE_STOPPING;
            stop_now = YES;
        } else {
            g_phase = PHASE_ACTIVE;
        }
    }
    pthread_mutex_unlock(&g_lock);
    if (stop_now) [c stopPictureInPicture];
}

- (void)pictureInPictureControllerDidStopPictureInPicture:(AVPictureInPictureController *)c {
    pthread_mutex_lock(&g_lock);
    if (self.retired) {
        [g_retired removeObject:self];
    } else if (self.gen == g_gen) {
        g_phase = PHASE_EXITED;
        g_stop_after_start = NO;
    }
    pthread_mutex_unlock(&g_lock);
}

- (void)pictureInPictureController:(AVPictureInPictureController *)c
    failedToStartPictureInPictureWithError:(NSError *)error {
    // error の内容は画面にもログにも出さない（A09）。
    pthread_mutex_lock(&g_lock);
    if (self.retired) {
        [g_retired removeObject:self];
    } else if (self.gen == g_gen) {
        // 開始中に利用者が取り消していたなら失敗ではなく Exited。
        g_phase = g_stop_after_start ? PHASE_EXITED : PHASE_FAILED_START;
        g_stop_after_start = NO;
    }
    pthread_mutex_unlock(&g_lock);
}

- (void)pictureInPictureController:(AVPictureInPictureController *)c
    restoreUserInterfaceForPictureInPictureStopWithCompletionHandler:(void (^)(BOOL))completionHandler {
    // 窓は常に残っているので復元は不要。completion は一度きり。
    completionHandler(YES);
}

// --- AVPictureInPictureSampleBufferPlaybackDelegate（ライブ扱い。再生・シークは持たない） ---

- (void)pictureInPictureController:(AVPictureInPictureController *)c setPlaying:(BOOL)playing {
}

- (CMTimeRange)pictureInPictureControllerTimeRangeForPlayback:(AVPictureInPictureController *)c {
    return CMTimeRangeMake(kCMTimeNegativeInfinity, kCMTimePositiveInfinity);
}

- (BOOL)pictureInPictureControllerIsPlaybackPaused:(AVPictureInPictureController *)c {
    return NO;
}

- (void)pictureInPictureController:(AVPictureInPictureController *)c
         didTransitionToRenderSize:(CMVideoDimensions)newRenderSize {
}

- (void)pictureInPictureController:(AVPictureInPictureController *)c
                    skipByInterval:(CMTime)skipInterval
                 completionHandler:(void (^)(void))completionHandler {
    completionHandler();
}

@end

// 単色の BGRA バッファを作る。失敗は NULL。
static CVPixelBufferRef make_color_buffer(uint8_t r, uint8_t g, uint8_t b, uint32_t w, uint32_t h) {
    NSDictionary *attrs = @{(id)kCVPixelBufferIOSurfacePropertiesKey : @{}};
    CVPixelBufferRef pb = NULL;
    if (CVPixelBufferCreate(kCFAllocatorDefault, w, h, kCVPixelFormatType_32BGRA,
                            (__bridge CFDictionaryRef)attrs, &pb) != kCVReturnSuccess ||
        !pb) {
        return NULL;
    }
    if (CVPixelBufferLockBaseAddress(pb, 0) != kCVReturnSuccess) {
        CVPixelBufferRelease(pb);
        return NULL;
    }
    uint8_t *base = (uint8_t *)CVPixelBufferGetBaseAddress(pb);
    size_t stride = CVPixelBufferGetBytesPerRow(pb);
    for (uint32_t y = 0; y < h; y++) {
        uint8_t *row = base + y * stride;
        for (uint32_t x = 0; x < w; x++) {
            row[x * 4 + 0] = b;
            row[x * 4 + 1] = g;
            row[x * 4 + 2] = r;
            row[x * 4 + 3] = 0xFF;
        }
    }
    CVPixelBufferUnlockBaseAddress(pb, 0);
    return pb;
}

// 1 フレーム enqueue する（main queue から呼ぶ）。世代が古ければ何もしない。
static void enqueue_frame(uint64_t gen) {
    pthread_mutex_lock(&g_lock);
    if (gen != g_gen || !g_layer || !g_format || g_buffer_count == 0) {
        pthread_mutex_unlock(&g_lock);
        return;
    }
    AVSampleBufferDisplayLayer *layer = g_layer;
    CVPixelBufferRef pb = g_buffers[g_tick++ % g_buffer_count];
    CVPixelBufferRetain(pb);
    CMVideoFormatDescriptionRef format = g_format;
    CFRetain(format);
    int64_t interval = g_interval_ms;
    pthread_mutex_unlock(&g_lock);

    CMSampleTimingInfo timing = {
        .duration = CMTimeMake(interval, 1000),
        .presentationTimeStamp = CMClockGetTime(CMClockGetHostTimeClock()),
        .decodeTimeStamp = kCMTimeInvalid,
    };
    CMSampleBufferRef sb = NULL;
    if (CMSampleBufferCreateReadyWithImageBuffer(kCFAllocatorDefault, pb, format, &timing, &sb) ==
            noErr &&
        sb) {
        CFArrayRef atts = CMSampleBufferGetSampleAttachmentsArray(sb, YES);
        if (atts && CFArrayGetCount(atts) > 0) {
            CFMutableDictionaryRef d = (CFMutableDictionaryRef)CFArrayGetValueAtIndex(atts, 0);
            CFDictionarySetValue(d, kCMSampleAttachmentKey_DisplayImmediately, kCFBooleanTrue);
        }
        if (layer.status == AVQueuedSampleBufferRenderingStatusFailed) [layer flush];
        if (layer.isReadyForMoreMediaData) [layer enqueueSampleBuffer:sb];
        CFRelease(sb);
    }
    CFRelease(format);
    CVPixelBufferRelease(pb);
}

int32_t fandhe_apple_pip_prepare(uint64_t gen, const uint8_t *rgb, size_t color_count,
                                 uint32_t interval_ms, uint32_t width, uint32_t height) {
    if (![NSThread isMainThread]) return RC_NOT_MAIN;
    @autoreleasepool {
        // 契約: 呼ぶたびに前の資源を先に解放する（失敗経路でも解放済みで返す）。
        release_all();
        if (@available(macOS 12.0, *)) {
            if (![AVPictureInPictureController isPictureInPictureSupported]) return RC_UNSUPPORTED;
        } else {
            return RC_UNSUPPORTED;
        }
        if (gen == 0 || rgb == NULL || color_count == 0 || color_count > MAX_COLORS ||
            interval_ms == 0 || width == 0 || height == 0 || width > MAX_DIMENSION ||
            height > MAX_DIMENSION) {
            return RC_INVALID;
        }
        if (@available(macOS 12.0, *)) {
            NSWindow *window = NSApp.mainWindow ?: NSApp.keyWindow ?: NSApp.windows.firstObject;
            NSView *content = window.contentView;
            if (!content) return RC_INTERNAL;

            pthread_mutex_lock(&g_lock);
            BOOL ok = YES;
            for (size_t i = 0; i < color_count; i++) {
                g_buffers[i] = make_color_buffer(rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2], width, height);
                if (!g_buffers[i]) {
                    ok = NO;
                    g_buffer_count = i;
                    break;
                }
                g_buffer_count = i + 1;
            }
            if (ok && CMVideoFormatDescriptionCreateForImageBuffer(kCFAllocatorDefault, g_buffers[0],
                                                                   &g_format) != noErr) {
                ok = NO;
            }
            if (!ok) {
                release_buffers_locked();
                pthread_mutex_unlock(&g_lock);
                return RC_INTERNAL;
            }
            g_interval_ms = interval_ms;
            g_tick = 0;
            g_gen = gen;
            g_phase = PHASE_PREPARING;
            g_stop_after_start = NO;
            pthread_mutex_unlock(&g_lock);

            // 窓の右下に固定する（窓のリサイズには追従しない）。操作は受けず、Metal の描画面の上に重ねる。
            content.wantsLayer = YES;
            AVSampleBufferDisplayLayer *layer = [[AVSampleBufferDisplayLayer alloc] init];
            layer.videoGravity = AVLayerVideoGravityResizeAspect;
            // layer の y 軸の向きは view の実装で変わる（実機では上原点だった）ので向きを見て下端へ寄せる。
            CGFloat y = content.layer.isGeometryFlipped
                            ? content.bounds.size.height - PREVIEW_H - PREVIEW_MARGIN
                            : PREVIEW_MARGIN;
            layer.frame = CGRectMake(content.bounds.size.width - PREVIEW_W - PREVIEW_MARGIN, y,
                                     PREVIEW_W, PREVIEW_H);
            layer.zPosition = 1000;
            [content.layer addSublayer:layer];

            FandhePipDelegate *delegate = [[FandhePipDelegate alloc] init];
            delegate.gen = gen;
            AVPictureInPictureControllerContentSource *source =
                [[AVPictureInPictureControllerContentSource alloc]
                    initWithSampleBufferDisplayLayer:layer
                                    playbackDelegate:delegate];
            AVPictureInPictureController *controller =
                [[AVPictureInPictureController alloc] initWithContentSource:source];
            if (!controller) {
                [layer removeFromSuperlayer];
                release_all();
                return RC_INTERNAL;
            }
            controller.delegate = delegate;

            dispatch_source_t timer =
                dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
            dispatch_source_set_timer(timer, dispatch_time(DISPATCH_TIME_NOW, interval_ms * NSEC_PER_MSEC),
                                      interval_ms * NSEC_PER_MSEC, 10 * NSEC_PER_MSEC);
            dispatch_source_set_event_handler(timer, ^{
                enqueue_frame(gen);
            });

            pthread_mutex_lock(&g_lock);
            g_layer = layer;
            g_delegate = delegate;
            g_controller = controller;
            g_timer = timer;
            pthread_mutex_unlock(&g_lock);
            dispatch_resume(timer);
            enqueue_frame(gen);
            return RC_OK;
        }
        return RC_UNSUPPORTED;
    }
}

int32_t fandhe_apple_pip_start(uint64_t gen) {
    if (![NSThread isMainThread]) return RC_NOT_MAIN;
    @autoreleasepool {
        pthread_mutex_lock(&g_lock);
        if (gen == 0 || gen != g_gen || !g_controller) {
            pthread_mutex_unlock(&g_lock);
            return RC_STALE;
        }
        if (g_phase == PHASE_STARTING || g_phase == PHASE_ACTIVE) {
            pthread_mutex_unlock(&g_lock);
            return RC_OK;
        }
        AVPictureInPictureController *controller = g_controller;
        if ((g_phase != PHASE_READY && g_phase != PHASE_EXITED) || !controller.isPictureInPicturePossible) {
            pthread_mutex_unlock(&g_lock);
            return RC_NOT_POSSIBLE;
        }
        g_phase = PHASE_STARTING;
        g_stop_after_start = NO;
        pthread_mutex_unlock(&g_lock);
        [controller startPictureInPicture];
        return RC_OK;
    }
}

int32_t fandhe_apple_pip_stop(uint64_t gen) {
    if (![NSThread isMainThread]) return RC_NOT_MAIN;
    @autoreleasepool {
        pthread_mutex_lock(&g_lock);
        if (gen == 0 || gen != g_gen || !g_controller) {
            pthread_mutex_unlock(&g_lock);
            return RC_STALE;
        }
        AVPictureInPictureController *controller = nil;
        if (g_phase == PHASE_STARTING) {
            g_stop_after_start = YES;
        } else if (g_phase == PHASE_ACTIVE) {
            g_phase = PHASE_STOPPING;
            controller = g_controller;
        }
        pthread_mutex_unlock(&g_lock);
        if (controller) [controller stopPictureInPicture];
        return RC_OK;
    }
}

void fandhe_apple_pip_snapshot(uint64_t *gen, int32_t *phase) {
    @autoreleasepool {
        pthread_mutex_lock(&g_lock);
        if (g_phase == PHASE_PREPARING && g_controller && [NSThread isMainThread] &&
            g_controller.isPictureInPicturePossible) {
            g_phase = PHASE_READY;
        }
        if (gen) *gen = g_gen;
        if (phase) *phase = g_phase;
        pthread_mutex_unlock(&g_lock);
    }
}

void fandhe_apple_pip_shutdown(void) {
    if ([NSThread isMainThread]) {
        @autoreleasepool {
            release_all();
        }
    } else {
        // main 以外からは AppKit に触れない。main への同期待ちはデッドロックするので非同期に積む。
        dispatch_async(dispatch_get_main_queue(), ^{
            @autoreleasepool {
                release_all();
            }
        });
    }
}

#endif  // TARGET_OS_OSX
