// カメラのライブ映像・フレーム取得: iOS・macOS 共有実装（iOS: Issue #813、macOS: Issue #1066、親 #168）。
// macOS は同じ AVFoundation 経路を共有し、iOS 専用の箇所（ズーム、中断の理由、背景遷移、UIKit）だけを
// `#if TARGET_OS_IPHONE` で分ける。macOS の差分: ズームは Rust 側が OS を呼ばずに `Unsupported` を返す
// （`videoZoomFactor` が無い。フレームは has_zoom = false）、中断はすべて Interrupted、背景停止なし、
// カメラの取り外し（`AVCaptureDeviceWasDisconnectedNotification`）を開始後は RuntimeFailed・開始前は
// StartFailed(カメラなし) で報告する。
// `AVCaptureSession` + `AVCaptureVideoDataOutput`（32BGRA）で背面カメラ（無ければ既定のカメラ）の
// フレームを受け、`videoZoomFactor` でズームする。
//
// `src/device/ios_camera_preview_stream.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS・macOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（AVFoundation / CoreMedia / CoreVideo をリンク）。
// 契約の SSOT は `fandhe_device::camera_preview_stream`。
//
// 契約（Rust → .m）:
//   fandhe_ios_camera_preview_stream_start(uint64_t id) -> int32_t
//     0 = 受理（許可の要求・構成・開始は session queue で非同期）、他 = 受理しない。
//   fandhe_ios_camera_preview_stream_set_zoom(uint64_t id, int32_t step) -> int32_t
//     0 = 受理（範囲の読み取り・適用は session queue で非同期。失敗は通知コード 6。
//     許可待ち・構成前でデバイスがまだ無いときも 6）、
//     他 = 現在の `id` と一致しない。step は 1 = 1 倍 / 2 = 2 倍。
//   fandhe_ios_camera_preview_stream_stop(uint64_t id)
//     停止・解放・実行中 delegate の終了待ちを完了してから戻る。`id` 不一致は何もしない。
// 契約（.m → Rust。`ios_camera_preview_stream.rs` の `#[no_mangle]`）:
//   fandhe_rust_camera_preview_stream_event(id, code, detail)
//     コードの表は `event_from_code`: 0 Running / 1 Interrupted / 2 Stopped(背景) / 3 Stopped(画面を離れた) /
//     4 StartFailed(detail 0 権限 / 1 カメラなし / 2 使用中 / 他 その他) / 5 RuntimeFailed /
//     6 ZoomFailed(detail 0 非対応 / 他 失敗)。
//   fandhe_rust_camera_preview_stream_frame(id, data, len, width, height, row_stride, zoom, has_zoom)
//     読み取りロック中のバッファの先頭。呼び出しの間だけ有効。縮小は Rust 側。
//   fandhe_rust_camera_preview_stream_zoom_target(step, min, max, out) -> int32_t
//     範囲の検証と目標値の決定（0 成功 / 1 非対応 / 2 失敗）。検証の SSOT は Rust 側。
//
// スレッド:
//   - sessionQueue（serial）: 構成・開始・停止・ズーム・通知の処理。現在のセッションの文脈
//     （`g_ctx`）への全アクセスはここに閉じる。
//   - videoQueue（serial）: sample buffer delegate 専用（公式でも serial 必須）。
//   - main からは `dispatch_sync(sessionQueue)`（stop）だけを行う。sessionQueue / videoQueue 上の処理から
//     main へ `dispatch_sync` はしない（デッドロック回避、契約表 11）。
//   - set_zoom の受理判定だけは queue に載せず、アトミックな `g_active_id` で行う（`startRunning` の
//     ブロック中に main を待たせないため）。
//
// 結果は OS の通知で確定し、タイマーや `startRunning` 直後の状態読みで推測しない。
// 開始・失敗の確定は DidStartRunning / RuntimeError / WasInterrupted の通知に一本化している。
//   - 開始: `AVCaptureSessionDidStartRunningNotification` で Running。
//   - 中断: `WasInterrupted`（背景由来 = `VideoDeviceNotAvailableInBackground` は背景停止、他は Interrupted）、
//     `InterruptionEnded` で Running へ戻る。
//   - 実行時エラー: `RuntimeError`。開始前なら StartFailed、開始後なら RuntimeFailed（自動の再開はしない）。
//   - 背景: `UIApplicationDidEnterBackgroundNotification`。許可待ちでも解放して Stopped(背景)。
//     `WillResignActive` は使わない（許可ダイアログで Inactive になり、正常な許可を中断扱いにするため）。
//   - 許可: `requestAccessForMediaType:` を常に呼ぶ（OS を正にする）。応答の後に `g_ctx` と cancelled を
//     再確認してから構成する。
// 終端（2・3・4・5）は `terminalSent` で 1 回だけ送る。送る前に文脈を解放側へ確定させる。
//
// 既知の天井: stop（Drop、UI スレッド）は `dispatch_sync(sessionQueue)` を待つため、sessionQueue が
// `startRunning`（同期ブロック）中だと画面離脱がその間止まる（実機で確認する）。
//
// 守る範囲: 失敗・カメラなし（シミュレータ）・権限拒否を成功へ畳まない、停止・失敗・背景の全経路で
// 入出力・購読・delegate を外す。守らない範囲: 自動の再開（中断の終了を除く）、前面・背面の切り替え、
// 露出・焦点、複数セッション、映像の向き。プロセス終了での解放は OS に任せる。
//
// ログは出さない（画素・NSError の文言を出さない。security.md A09）。AVFoundation 呼び出しは
// `@autoreleasepool` の内側で行う。画素は保存・送信しない。

#import <AVFoundation/AVFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#if TARGET_OS_IPHONE
#import <UIKit/UIKit.h>
#endif
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>

extern void fandhe_rust_camera_preview_stream_event(uint64_t id, int32_t code, int32_t detail);
extern void fandhe_rust_camera_preview_stream_frame(uint64_t id, const uint8_t *data, size_t len,
                                                    uint32_t width, uint32_t height,
                                                    size_t row_stride, float zoom, bool has_zoom);
extern int32_t fandhe_rust_camera_preview_stream_zoom_target(int32_t step, float min, float max,
                                                             float *out);

// イベントコード（`event_from_code` の表）。
enum {
    kEvRunning = 0,
    kEvInterrupted = 1,
    kEvStoppedBackground = 2,
    kEvStartFailed = 4,
    kEvRuntimeFailed = 5,
    kEvZoomFailed = 6,
};
// StartFailed の detail。
enum {
    kDetailPermission = 0,
    kDetailNoCamera = 1,
    kDetailInUse = 2,
    kDetailOther = 3,
};

static dispatch_queue_t sessionQueue(void) {
    static dispatch_queue_t q;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        q = dispatch_queue_create("com.fandhe.camera-preview.session", DISPATCH_QUEUE_SERIAL);
    });
    return q;
}

static dispatch_queue_t videoQueue(void) {
    static dispatch_queue_t q;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        q = dispatch_queue_create("com.fandhe.camera-preview.video", DISPATCH_QUEUE_SERIAL);
    });
    return q;
}

// ---- delegate（videoQueue で呼ばれる。自分の id とデバイスだけを持つ）---------------------------

@interface FandheCamPreviewDelegate : NSObject <AVCaptureVideoDataOutputSampleBufferDelegate>
@property(nonatomic, assign) uint64_t cid;
@property(nonatomic, strong) AVCaptureDevice *device;
@end

@implementation FandheCamPreviewDelegate

- (void)captureOutput:(AVCaptureOutput *)output
    didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
           fromConnection:(AVCaptureConnection *)connection {
    @autoreleasepool {
        CVImageBufferRef image = CMSampleBufferGetImageBuffer(sampleBuffer);
        if (image == NULL) {
            return;
        }
        if (CVPixelBufferGetPixelFormatType(image) != kCVPixelFormatType_32BGRA) {
            return;
        }
        if (CVPixelBufferLockBaseAddress(image, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
            return;
        }
        size_t width = CVPixelBufferGetWidth(image);
        size_t height = CVPixelBufferGetHeight(image);
        size_t stride = CVPixelBufferGetBytesPerRow(image);
        const uint8_t *base = (const uint8_t *)CVPixelBufferGetBaseAddress(image);
        size_t len = 0;
        if (base != NULL && width <= UINT32_MAX && height <= UINT32_MAX &&
            !__builtin_mul_overflow(stride, height, &len)) {
#if TARGET_OS_IPHONE
            float zoom = (float)self.device.videoZoomFactor;
            bool hasZoom = true;
#else
            // macOS にはズーム倍率が無い（値を捏造しない）。
            float zoom = 0;
            bool hasZoom = false;
#endif
            fandhe_rust_camera_preview_stream_frame(self.cid, base, len, (uint32_t)width,
                                                    (uint32_t)height, stride, zoom, hasZoom);
        }
        CVPixelBufferUnlockBaseAddress(image, kCVPixelBufferLock_ReadOnly);
    }
}

@end

// ---- セッションの文脈（sessionQueue に閉じる）-------------------------------------------------

@interface FandheCamPreviewCtx : NSObject
@property(nonatomic, assign) uint64_t cid;
@property(nonatomic, strong) AVCaptureSession *session;
@property(nonatomic, strong) AVCaptureDevice *device;
@property(nonatomic, strong) AVCaptureVideoDataOutput *output;
@property(nonatomic, strong) FandheCamPreviewDelegate *delegate;
@property(nonatomic, strong) NSMutableArray *observers;
@property(nonatomic, assign) BOOL cancelled;
@property(nonatomic, assign) BOOL reportedRunning;
@property(nonatomic, assign) BOOL terminalSent;
@end

@implementation FandheCamPreviewCtx
@end

static FandheCamPreviewCtx *g_ctx = nil;       // sessionQueue のみ
static _Atomic uint64_t g_active_id = 0;       // set_zoom の受理判定用

// 購読・入出力・delegate を外して文脈を捨てる（sessionQueue 上）。
static void teardown(FandheCamPreviewCtx *ctx) {
    ctx.cancelled = YES;
    for (id token in ctx.observers) {
        [[NSNotificationCenter defaultCenter] removeObserver:token];
    }
    [ctx.observers removeAllObjects];
    @autoreleasepool {
        [ctx.output setSampleBufferDelegate:nil queue:NULL];
        if (ctx.session != nil) {
            [ctx.session stopRunning];
            for (AVCaptureInput *input in [ctx.session.inputs copy]) {
                [ctx.session removeInput:input];
            }
            for (AVCaptureOutput *out in [ctx.session.outputs copy]) {
                [ctx.session removeOutput:out];
            }
        }
    }
    ctx.delegate = nil;
    ctx.output = nil;
    ctx.session = nil;
    ctx.device = nil;
    uint64_t expected = ctx.cid;
    atomic_compare_exchange_strong(&g_active_id, &expected, 0);
    if (g_ctx == ctx) {
        g_ctx = nil;
    }
}

// 終端を 1 回だけ送る。送る前に文脈を解放側へ確定させる（sessionQueue 上）。
static void send_terminal(FandheCamPreviewCtx *ctx, int32_t code, int32_t detail) {
    if (ctx.terminalSent || ctx.cancelled) {
        return;
    }
    ctx.terminalSent = YES;
    teardown(ctx);
    fandhe_rust_camera_preview_stream_event(ctx.cid, code, detail);
}

// AVError を StartFailed の detail へ写す（1 か所。同じ意味のコードを全部拾い、-1 等へ潰さない）。
static int32_t map_av_error(NSError *error) {
    if (error == nil || ![error.domain isEqualToString:AVFoundationErrorDomain]) {
        return kDetailOther;
    }
    switch (error.code) {
        case AVErrorApplicationIsNotAuthorized:
        case AVErrorApplicationIsNotAuthorizedToUseDevice:
            return kDetailPermission;
        case AVErrorDeviceNotConnected:
            return kDetailNoCamera;
        case AVErrorDeviceAlreadyUsedByAnotherSession:
        case AVErrorDeviceInUseByAnotherApplication:
            return kDetailInUse;
        default:
            return kDetailOther;
    }
}

static AVCaptureDevice *pick_device(void) {
    AVCaptureDevice *dev =
        [AVCaptureDevice defaultDeviceWithDeviceType:AVCaptureDeviceTypeBuiltInWideAngleCamera
                                           mediaType:AVMediaTypeVideo
                                            position:AVCaptureDevicePositionBack];
    if (dev == nil) {
        dev = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeVideo];
    }
    return dev;
}

static void observe(FandheCamPreviewCtx *ctx, NSNotificationName name, id object,
                    void (^handler)(NSNotification *note)) {
    id token = [[NSNotificationCenter defaultCenter]
        addObserverForName:name
                    object:object
                     queue:nil
                usingBlock:^(NSNotification *note __unused) {
                    dispatch_async(sessionQueue(), ^{
                        if (g_ctx != ctx || ctx.cancelled) {
                            return;
                        }
                        handler(note);
                    });
                }];
    [ctx.observers addObject:token];
}

// 構成して開始する（sessionQueue 上、許可の応答の後）。
static void configure_and_start(FandheCamPreviewCtx *ctx) {
    @autoreleasepool {
        AVCaptureDevice *device = pick_device();
        if (device == nil) {
            send_terminal(ctx, kEvStartFailed, kDetailNoCamera);
            return;
        }
        NSError *error = nil;
        AVCaptureDeviceInput *input = [AVCaptureDeviceInput deviceInputWithDevice:device
                                                                            error:&error];
        if (input == nil) {
            send_terminal(ctx, kEvStartFailed, map_av_error(error));
            return;
        }
        AVCaptureSession *session = [[AVCaptureSession alloc] init];
        ctx.session = session;
        ctx.device = device;

        [session beginConfiguration];
        if ([session canSetSessionPreset:AVCaptureSessionPreset640x480]) {
            session.sessionPreset = AVCaptureSessionPreset640x480;
        }
        if (![session canAddInput:input]) {
            [session commitConfiguration];
            send_terminal(ctx, kEvStartFailed, kDetailOther);
            return;
        }
        [session addInput:input];

        AVCaptureVideoDataOutput *output = [[AVCaptureVideoDataOutput alloc] init];
        if (![session canAddOutput:output]) {
            [session commitConfiguration];
            send_terminal(ctx, kEvStartFailed, kDetailOther);
            return;
        }
        [session addOutput:output];
        // 対応する画素形式の一覧は接続先のデバイスに依存し、セッションへ繋ぐ前は空になる。
        // 接続した後の OS の一覧で BGRA を確かめる。
        BOOL bgraOk = NO;
        for (NSNumber *fmt in output.availableVideoCVPixelFormatTypes) {
            if (fmt.unsignedIntValue == kCVPixelFormatType_32BGRA) {
                bgraOk = YES;
                break;
            }
        }
        if (!bgraOk) {
            [session commitConfiguration];
            send_terminal(ctx, kEvStartFailed, kDetailOther);
            return;
        }
        output.videoSettings = @{(id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA)};
        output.alwaysDiscardsLateVideoFrames = YES;
        FandheCamPreviewDelegate *delegate = [[FandheCamPreviewDelegate alloc] init];
        delegate.cid = ctx.cid;
        delegate.device = device;
        [output setSampleBufferDelegate:delegate queue:videoQueue()];
        [session commitConfiguration];
        ctx.output = output;
        ctx.delegate = delegate;

        // 通知の購読（session ごとに object を指定し、teardown で外す）。
        observe(ctx, AVCaptureSessionDidStartRunningNotification, ctx.session, ^(NSNotification *note __unused) {
            ctx.reportedRunning = YES;
            fandhe_rust_camera_preview_stream_event(ctx.cid, kEvRunning, 0);
        });
        observe(ctx, AVCaptureSessionWasInterruptedNotification, ctx.session, ^(NSNotification *note __unused) {
#if TARGET_OS_IPHONE
            NSNumber *reason = note.userInfo[AVCaptureSessionInterruptionReasonKey];
            if (reason != nil && reason.integerValue ==
                                     AVCaptureSessionInterruptionReasonVideoDeviceNotAvailableInBackground) {
                send_terminal(ctx, kEvStoppedBackground, 0);
                return;
            }
#endif
            // macOS は中断の理由を取れないため、すべて Interrupted（背景停止はない）。
            fandhe_rust_camera_preview_stream_event(ctx.cid, kEvInterrupted, 0);
        });
        observe(ctx, AVCaptureSessionInterruptionEndedNotification, ctx.session, ^(NSNotification *note __unused) {
            ctx.reportedRunning = YES;
            fandhe_rust_camera_preview_stream_event(ctx.cid, kEvRunning, 0);
        });
        observe(ctx, AVCaptureSessionRuntimeErrorNotification, ctx.session, ^(NSNotification *note __unused) {
            if (ctx.reportedRunning) {
                send_terminal(ctx, kEvRuntimeFailed, 0);
            } else {
                NSError *err = note.userInfo[AVCaptureSessionErrorKey];
                send_terminal(ctx, kEvStartFailed, map_av_error(err));
            }
        });
#if !TARGET_OS_IPHONE
        // カメラの取り外し（macOS）。開始後は RuntimeFailed、開始前は StartFailed(カメラなし)。
        observe(ctx, AVCaptureDeviceWasDisconnectedNotification, device, ^(NSNotification *note __unused) {
            if (ctx.reportedRunning) {
                send_terminal(ctx, kEvRuntimeFailed, 0);
            } else {
                send_terminal(ctx, kEvStartFailed, kDetailNoCamera);
            }
        });
#endif

        // 開始・失敗の確定は上の通知だけで行う。`startRunning` の戻り後に `isRunning` を読んで
        // 失敗と推測しない（同期ブロック中に届いた RuntimeError の写像を潰すため）。
        [session startRunning];
    }
}

// 背景遷移の購読（iOS のみ。1 回だけ登録。main で受けて queue へ移す）。
#if TARGET_OS_IPHONE
static void ensure_background_observer(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        // 登録トークンは通知センターが握る（アプリの存続中は外さない）。
        [[NSNotificationCenter defaultCenter]
            addObserverForName:UIApplicationDidEnterBackgroundNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *note __unused) {
                        dispatch_async(sessionQueue(), ^{
                            FandheCamPreviewCtx *ctx = g_ctx;
                            if (ctx != nil) {
                                send_terminal(ctx, kEvStoppedBackground, 0);
                            }
                        });
                    }];
    });
}
#endif

int32_t fandhe_ios_camera_preview_stream_start(uint64_t id) {
    if (id == 0) {
        return 1;
    }
#if TARGET_OS_IPHONE
    ensure_background_observer();
#endif
    atomic_store(&g_active_id, id);
    dispatch_async(sessionQueue(), ^{
        // 前の文脈が残っていれば通知なしで解放する（呼び出し側は 1 件ずつ止めてから始める契約）。
        if (g_ctx != nil) {
            teardown(g_ctx);
        }
        FandheCamPreviewCtx *ctx = [[FandheCamPreviewCtx alloc] init];
        ctx.cid = id;
        ctx.observers = [NSMutableArray array];
        g_ctx = ctx;
        // 許可は常に OS へ問う（決定済みなら即座に応答が来る）。
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo
                                 completionHandler:^(BOOL granted) {
                                     dispatch_async(sessionQueue(), ^{
                                         // 応答待ちの間に止められた・背景へ移った場合は何もしない。
                                         if (g_ctx != ctx || ctx.cancelled) {
                                             return;
                                         }
                                         if (!granted) {
                                             send_terminal(ctx, kEvStartFailed, kDetailPermission);
                                             return;
                                         }
                                         configure_and_start(ctx);
                                     });
                                 }];
    });
    return 0;
}

#if TARGET_OS_IPHONE
int32_t fandhe_ios_camera_preview_stream_set_zoom(uint64_t id, int32_t step) {
    if (id == 0 || atomic_load(&g_active_id) != id) {
        return 1;
    }
    dispatch_async(sessionQueue(), ^{
        FandheCamPreviewCtx *ctx = g_ctx;
        // 破棄済み（id 不一致・cancelled）は Rust が登録解除済みなので黙って捨てる。
        if (ctx == nil || ctx.cid != id || ctx.cancelled) {
            return;
        }
        // 受理済みの要求を黙って捨てない。許可待ち・構成前でまだ OS のデバイスが無いときは失敗で返す。
        if (ctx.device == nil) {
            fandhe_rust_camera_preview_stream_event(id, kEvZoomFailed, 1);
            return;
        }
        @autoreleasepool {
            AVCaptureDevice *device = ctx.device;
            float target = 0;
            int32_t code = fandhe_rust_camera_preview_stream_zoom_target(
                step, (float)device.minAvailableVideoZoomFactor,
                (float)device.maxAvailableVideoZoomFactor, &target);
            if (code != 0) {
                fandhe_rust_camera_preview_stream_event(id, kEvZoomFailed, code == 1 ? 0 : 1);
                return;
            }
            NSError *error = nil;
            if (![device lockForConfiguration:&error]) {
                fandhe_rust_camera_preview_stream_event(id, kEvZoomFailed, 1);
                return;
            }
            device.videoZoomFactor = target;
            [device unlockForConfiguration];
        }
    });
    return 0;
}
#endif

void fandhe_ios_camera_preview_stream_stop(uint64_t id) {
    uint64_t expected = id;
    atomic_compare_exchange_strong(&g_active_id, &expected, 0);
    dispatch_sync(sessionQueue(), ^{
        FandheCamPreviewCtx *ctx = g_ctx;
        if (ctx != nil && ctx.cid == id) {
            teardown(ctx);
        }
    });
    // 実行中の delegate の終了を待つ（delegate は teardown で外し済みなので新規は来ない）。
    dispatch_sync(videoQueue(), ^{
    });
}
