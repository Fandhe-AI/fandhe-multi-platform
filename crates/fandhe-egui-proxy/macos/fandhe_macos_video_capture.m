// macOS 向け動画の撮影（AppKit の撮影パネル + AVCaptureSession / AVCaptureMovieFileOutput、Issue #1071、親 #169）。
// macOS には OS 標準の動画撮影 UI が無いため、アプリ側で NSPanel（プレビュー + 3 ボタン）を組む。
// `src/device/macos_video_capture.rs` から呼ばれ、`build.rs` の `build_macos()` が feature `device` 有効時に
// コンパイルする（AppKit は既存のリンク。AVFoundation・CoreMedia・QuartzCore は明示リンク）。
// 映像のみ録る（マイクは使わず、`NSMicrophoneUsageDescription` にも触れない）。
// カメラの宣言は `macos/Info.plist` の `NSCameraUsageDescription`（文言に機能名を足すだけ）。
//
// C ABI（Rust 側の `// SAFETY:` の根拠）:
// - `int32_t fandhe_macos_video_capture_start(uint64_t request_id, uint32_t max_duration_secs)`
//   戻り値 0 = 受理（結果は後で `fandhe_rust_video_capture_complete` から 1 回だけ届く）/ それ以外 = 内部エラー。
// - Rust 側の受け口（iOS と同名・同シグネチャ。cfg が排他なのでリンクは衝突しない）:
//   `int32_t fandhe_rust_video_capture_try_begin_present(uint64_t request_id)`（1 = 表示してよい。0 なら
//   表示も完了の送信もしない）、
//   `void fandhe_rust_video_capture_complete(uint64_t request_id, int32_t kind, uint64_t size_bytes,
//   int64_t duration_ms, int32_t width, int32_t height, int32_t rotation_deg)`（1 要求につき 1 回）。
//   ポインタは渡さない。パス・NSError の文言は Rust へ渡さない。
//
// kind（`fandhe_device::video_capture::result_from_code` の表と一致させる）:
// 0 = 撮れた / 1 = 利用者が閉じた・キャンセル / 2 = 非対応（カメラなし）/ 3 = カメラ権限の拒否・制限 /
// 4・5・6 = macOS では使わない（撮影アプリなし・前面判定・表示前の中断は持たない）/
// 7 = 0 バイト・動画なし / 8 = メタデータ（映像トラック・寸法）を読めない / 9 = その他（置き場の削除失敗を含む）。
// kind 0 以外ではメタデータ引数は 0。duration_ms = -1 は「長さなし」（失敗にしない）。
// rotation_deg は preferredTransform から求めた 0..360 の整数。
//
// 並行性: 状態はすべて直列 queue `g_queue` だけが触る（ボタン・閉じる・delegate・通知・start は全部ここへ
// dispatch_async で流す）。UI（パネルの生成・表示・閉じる・ボタンの有効化）は main queue。main への
// dispatch_sync は使わず、main から queue へ戻るのも async。出口 `FandheMacVCFinish` が唯一の完了経路。
// 期限・タイマーは持たない。上限秒数は `maxRecordedDuration` で OS に任せる（上限到達は撮れた扱い）。
// 一時ファイルは要求ごとに mkdtemp（0700）で作った置き場の `clip.mov` だけ。読み取りが終わるまで消さず、
// 出口で置き場ごと消す。消せなかったら kind 9 にし、置き場を `g_leftover`（要求より長く生きる static）へ
// 記録して次の要求の開始時に消し直す。消し直せなければ次の要求も 9 にする。
// 動画の中身・パス・NSError の文言はログに出さない（NSLog は使わない）。
// 守らない範囲: パネルの見た目・画質・形式、カメラの選択（OS の既定）、他アプリとの同時利用、
// 撮影中のプロセス死、複数プロセス、閉じた通知が来ない場合の復旧。

#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <QuartzCore/QuartzCore.h>
#include <math.h>
#include <stdint.h>

extern int32_t fandhe_rust_video_capture_try_begin_present(uint64_t request_id);
extern void fandhe_rust_video_capture_complete(uint64_t request_id, int32_t kind, uint64_t size_bytes,
                                               int64_t duration_ms, int32_t width, int32_t height,
                                               int32_t rotation_deg);

typedef NS_ENUM(int, FandheMacVCPhase) {
    FandheMacVCPhasePreparing,
    FandheMacVCPhasePreviewing,
    FandheMacVCPhaseRecording,
    FandheMacVCPhaseFinishing,
};

// 画面側の部品。UI の操作は main、ボタンの受け口は queue へ流す。
@interface FandheMacVCController : NSObject <AVCaptureFileOutputRecordingDelegate, NSWindowDelegate>
@property(nonatomic) uint64_t requestId;
// 以下は main queue だけが触る。
@property(nonatomic, strong) NSPanel *panel;
@property(nonatomic, strong) AVCaptureVideoPreviewLayer *previewLayer;
@property(nonatomic, strong) NSButton *startButton;
@property(nonatomic, strong) NSButton *stopButton;
@property(nonatomic, strong) NSButton *cancelButton;
@property(nonatomic) BOOL closed;
@end

// ---- queue だけが触る状態 ----
static dispatch_queue_t g_queue;
static BOOL g_active = NO;
static uint64_t g_rid = 0;
static FandheMacVCPhase g_phase = FandheMacVCPhasePreparing;
static BOOL g_cancelled = NO;
static AVCaptureSession *g_session = nil;
static AVCaptureMovieFileOutput *g_output = nil;
static NSURL *g_dir = nil;
static NSMutableArray *g_tokens = nil;
static FandheMacVCController *g_ctl = nil;
// 前回消せなかった置き場。要求より長く生きる。
static NSURL *g_leftover = nil;

static BOOL FandheMacVCIsCurrent(uint64_t rid) { return g_active && g_rid == rid; }

// 出口の後半（queue）。置き場を消し、状態を空け、結果を 1 回だけ送る。
static void FandheMacVCFinishTail(uint64_t rid, int32_t kind, uint64_t size, int64_t dur, int32_t w, int32_t h,
                                  int32_t rot, BOOL send) {
    if (g_dir != nil) {
        NSFileManager *fm = [NSFileManager defaultManager];
        // すでに無いなら解放済みと同じ。残っているのに消せなかった場合だけ未解放として追跡する。
        if (![fm removeItemAtURL:g_dir error:nil] && [fm fileExistsAtPath:g_dir.path]) {
            g_leftover = g_dir;
            kind = 9;
            size = 0;
            dur = 0;
            w = 0;
            h = 0;
            rot = 0;
        }
        g_dir = nil;
    }
    g_active = NO;
    if (send) {
        fandhe_rust_video_capture_complete(rid, kind, size, dur, w, h, rot);
    }
}

// 唯一の出口（queue 上）。セッションを止めて UI を畳み、置き場を消してから結果を送る。
static void FandheMacVCFinish(uint64_t rid, int32_t kind, uint64_t size, int64_t dur, int32_t w, int32_t h,
                              int32_t rot, BOOL send) {
    if (!FandheMacVCIsCurrent(rid)) {
        return;
    }
    g_phase = FandheMacVCPhaseFinishing;
    if (g_session != nil) {
        [g_session stopRunning];
    }
    for (id token in g_tokens) {
        [[NSNotificationCenter defaultCenter] removeObserver:token];
    }
    g_tokens = nil;
    g_session = nil;
    g_output = nil;
    FandheMacVCController *ctl = g_ctl;
    g_ctl = nil;
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            ctl.closed = YES;
            [ctl.previewLayer removeFromSuperlayer];
            ctl.previewLayer = nil;
            ctl.panel.delegate = nil;
            [ctl.panel orderOut:nil];
            [ctl.panel close];
            ctl.panel = nil;
            dispatch_async(g_queue, ^{
                @autoreleasepool {
                    FandheMacVCFinishTail(rid, kind, size, dur, w, h, rot, send);
                }
            });
        }
    });
}

static void FandheMacVCFinishKind(uint64_t rid, int32_t kind) { FandheMacVCFinish(rid, kind, 0, 0, 0, 0, 0, YES); }

// 録画済みファイルからサイズ・長さ・寸法・回転を読んで出口へ渡す（読み終えるまでファイルは消さない）。
static void FandheMacVCRead(uint64_t rid, NSURL *url) {
    NSNumber *fileSize = nil;
    [url getResourceValue:&fileSize forKey:NSURLFileSizeKey error:nil];
    uint64_t size = fileSize != nil ? fileSize.unsignedLongLongValue : 0;
    if (size == 0) {
        FandheMacVCFinishKind(rid, 7);
        return;
    }
    AVURLAsset *asset = [AVURLAsset URLAssetWithURL:url options:nil];
    [asset loadValuesAsynchronouslyForKeys:@[ @"duration", @"tracks" ]
                         completionHandler:^{
                           @autoreleasepool {
                               int64_t durMs = -1;
                               NSError *e1 = nil;
                               NSError *e2 = nil;
                               if ([asset statusOfValueForKey:@"duration" error:&e1] == AVKeyValueStatusLoaded) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
                                   CMTime d = asset.duration;
#pragma clang diagnostic pop
                                   if (CMTIME_IS_NUMERIC(d) && d.timescale > 0 && d.value >= 0) {
                                       durMs = (int64_t)CMTimeConvertScale(d, 1000, kCMTimeRoundingMethod_Default).value;
                                   }
                               }
                               AVAssetTrack *track = nil;
                               if ([asset statusOfValueForKey:@"tracks" error:&e2] == AVKeyValueStatusLoaded) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
                                   track = [asset tracksWithMediaType:AVMediaTypeVideo].firstObject;
#pragma clang diagnostic pop
                               }
                               if (track == nil) {
                                   dispatch_async(g_queue, ^{ FandheMacVCFinishKind(rid, 8); });
                                   return;
                               }
                               [track loadValuesAsynchronouslyForKeys:@[ @"naturalSize", @"preferredTransform" ]
                                                    completionHandler:^{
                                                      @autoreleasepool {
                                                          NSError *e3 = nil;
                                                          NSError *e4 = nil;
                                                          if ([track statusOfValueForKey:@"naturalSize" error:&e3] !=
                                                                  AVKeyValueStatusLoaded ||
                                                              [track statusOfValueForKey:@"preferredTransform"
                                                                                   error:&e4] !=
                                                                  AVKeyValueStatusLoaded) {
                                                              dispatch_async(g_queue, ^{ FandheMacVCFinishKind(rid, 8); });
                                                              return;
                                                          }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
                                                          CGSize natural = track.naturalSize;
                                                          CGAffineTransform t = track.preferredTransform;
#pragma clang diagnostic pop
                                                          double deg = atan2(t.b, t.a) * 180.0 / M_PI;
                                                          long r = lround(deg);
                                                          r = ((r % 360) + 360) % 360;
                                                          int32_t w = (int32_t)lround(natural.width);
                                                          int32_t h = (int32_t)lround(natural.height);
                                                          dispatch_async(g_queue, ^{
                                                            FandheMacVCFinish(rid, 0, size, durMs, w, h, (int32_t)r, YES);
                                                          });
                                                      }
                                                    }];
                           }
                         }];
}

// 前回消せなかった置き場を消し直す（queue）。消せなければ NO。
static BOOL FandheMacVCRetryLeftover(void) {
    if (g_leftover == nil) {
        return YES;
    }
    NSFileManager *fm = [NSFileManager defaultManager];
    if ([fm removeItemAtURL:g_leftover error:nil] || ![fm fileExistsAtPath:g_leftover.path]) {
        g_leftover = nil;
        return YES;
    }
    return NO;
}

@implementation FandheMacVCController
- (void)startPressed:(id)sender {
    uint64_t rid = self.requestId;
    dispatch_async(g_queue, ^{
        @autoreleasepool {
            if (!FandheMacVCIsCurrent(rid) || g_phase != FandheMacVCPhasePreviewing || g_output == nil) {
                return;
            }
            NSString *tmpl = [NSTemporaryDirectory() stringByAppendingPathComponent:@"fandhe-video-capture.XXXXXX"];
            char buf[PATH_MAX];
            if (strlcpy(buf, tmpl.fileSystemRepresentation, sizeof(buf)) >= sizeof(buf) || mkdtemp(buf) == NULL) {
                FandheMacVCFinishKind(rid, 9);
                return;
            }
            g_dir = [NSURL fileURLWithFileSystemRepresentation:buf isDirectory:YES relativeToURL:nil];
            g_phase = FandheMacVCPhaseRecording;
            FandheMacVCController *ctl = g_ctl;
            dispatch_async(dispatch_get_main_queue(), ^{
                ctl.startButton.enabled = NO;
                ctl.stopButton.enabled = YES;
            });
            [g_output startRecordingToOutputFileURL:[g_dir URLByAppendingPathComponent:@"clip.mov"]
                                  recordingDelegate:ctl];
        }
    });
}
- (void)stopPressed:(id)sender {
    uint64_t rid = self.requestId;
    dispatch_async(g_queue, ^{
        @autoreleasepool {
            if (!FandheMacVCIsCurrent(rid) || g_phase != FandheMacVCPhaseRecording) {
                return;
            }
            g_phase = FandheMacVCPhaseFinishing;
            [self disableAll];
            [g_output stopRecording];
        }
    });
}
- (void)cancelPressed:(id)sender {
    uint64_t rid = self.requestId;
    dispatch_async(g_queue, ^{
        @autoreleasepool {
            if (!FandheMacVCIsCurrent(rid)) {
                return;
            }
            if (g_phase == FandheMacVCPhaseRecording) {
                g_cancelled = YES;
                g_phase = FandheMacVCPhaseFinishing;
                [self disableAll];
                [g_output stopRecording];
            } else if (g_phase == FandheMacVCPhasePreparing || g_phase == FandheMacVCPhasePreviewing) {
                FandheMacVCFinishKind(rid, 1);
            }
        }
    });
}
- (void)disableAll {
    dispatch_async(dispatch_get_main_queue(), ^{
        self.startButton.enabled = NO;
        self.stopButton.enabled = NO;
        self.cancelButton.enabled = NO;
    });
}
// 閉じるボタンは常に自前のキャンセルへ流す（パネルは出口で畳む）。
- (BOOL)windowShouldClose:(NSWindow *)sender {
    [self cancelPressed:nil];
    return NO;
}
// スレッドは不定。queue へ移す。
- (void)captureOutput:(AVCaptureFileOutput *)output
    didFinishRecordingToOutputFileAtURL:(NSURL *)outputFileURL
                        fromConnections:(NSArray<AVCaptureConnection *> *)connections
                                  error:(NSError *)error {
    uint64_t rid = self.requestId;
    BOOL noData = error != nil && error.code == AVErrorNoDataCaptured;
    // 上限到達などは error があっても SuccessfullyFinished が YES で、撮れた扱い。
    BOOL finishedOk = error == nil || [error.userInfo[AVErrorRecordingSuccessfullyFinishedKey] boolValue];
    dispatch_async(g_queue, ^{
        @autoreleasepool {
            if (!FandheMacVCIsCurrent(rid) ||
                (g_phase != FandheMacVCPhaseRecording && g_phase != FandheMacVCPhaseFinishing)) {
                return;
            }
            g_phase = FandheMacVCPhaseFinishing;
            [self disableAll];
            if (g_cancelled) {
                FandheMacVCFinishKind(rid, 1);
            } else if (noData) {
                FandheMacVCFinishKind(rid, 7);
            } else if (!finishedOk) {
                FandheMacVCFinishKind(rid, 9);
            } else {
                FandheMacVCRead(rid, outputFileURL);
            }
        }
    });
}
@end

static NSButton *FandheMacVCButton(NSString *title, FandheMacVCController *ctl, SEL action, CGFloat x) {
    NSButton *b = [NSButton buttonWithTitle:title target:ctl action:action];
    b.frame = NSMakeRect(x, 8, 160, 28);
    return b;
}

// main: パネルを組んで出す。出す直前に Rust へ現在の要求かを確かめる。
static void FandheMacVCPresent(uint64_t rid, FandheMacVCController *ctl, AVCaptureSession *session) {
    if (NSApp == nil) {
        dispatch_async(g_queue, ^{ FandheMacVCFinishKind(rid, 9); });
        return;
    }
    if (fandhe_rust_video_capture_try_begin_present(rid) == 0) {
        // 古い要求は表示も完了の送信もしない。後片付けだけ。
        dispatch_async(g_queue, ^{ FandheMacVCFinish(rid, 0, 0, 0, 0, 0, 0, NO); });
        return;
    }
    if (ctl.closed) {
        return;
    }
    NSPanel *panel = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, 640, 520)
                                                styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable
                                                  backing:NSBackingStoreBuffered
                                                    defer:NO];
    panel.title = @"動画の撮影";
    panel.releasedWhenClosed = NO;
    panel.delegate = ctl;
    NSView *content = panel.contentView;
    content.wantsLayer = YES;
    AVCaptureVideoPreviewLayer *layer = [AVCaptureVideoPreviewLayer layerWithSession:session];
    layer.frame = CGRectMake(0, 44, 640, 476);
    layer.videoGravity = AVLayerVideoGravityResizeAspect;
    layer.autoresizingMask = kCALayerWidthSizable | kCALayerHeightSizable;
    [content.layer addSublayer:layer];
    ctl.previewLayer = layer;
    ctl.startButton = FandheMacVCButton(@"録画を始める", ctl, @selector(startPressed:), 120);
    ctl.stopButton = FandheMacVCButton(@"止める", ctl, @selector(stopPressed:), 290);
    ctl.stopButton.enabled = NO;
    ctl.cancelButton = FandheMacVCButton(@"キャンセル", ctl, @selector(cancelPressed:), 460);
    for (NSButton *b in @[ ctl.startButton, ctl.stopButton, ctl.cancelButton ]) {
        [content addSubview:b];
    }
    ctl.panel = panel;
    [panel center];
    [panel makeKeyAndOrderFront:nil];
    dispatch_async(g_queue, ^{
        @autoreleasepool {
            if (FandheMacVCIsCurrent(rid) && g_phase == FandheMacVCPhasePreparing && g_session == session) {
                [session startRunning];
                g_phase = FandheMacVCPhasePreviewing;
            }
        }
    });
}

// 許可のあと、デバイス確認とセッション構成をして main へ渡す（queue）。
static void FandheMacVCPrepare(uint64_t rid, uint32_t maxSecs) {
    AVCaptureDevice *device = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeVideo];
    if (device == nil) {
        FandheMacVCFinishKind(rid, 2);
        return;
    }
    NSError *err = nil;
    AVCaptureDeviceInput *input = [AVCaptureDeviceInput deviceInputWithDevice:device error:&err];
    AVCaptureSession *session = [[AVCaptureSession alloc] init];
    AVCaptureMovieFileOutput *output = [[AVCaptureMovieFileOutput alloc] init];
    if (input == nil || ![session canAddInput:input]) {
        FandheMacVCFinishKind(rid, 9);
        return;
    }
    [session addInput:input];
    if (![session canAddOutput:output]) {
        FandheMacVCFinishKind(rid, 9);
        return;
    }
    [session addOutput:output];
    output.maxRecordedDuration = CMTimeMakeWithSeconds((Float64)maxSecs, 600);
    g_session = session;
    g_output = output;
    g_tokens = [NSMutableArray array];
    void (^onFailure)(NSNotification *) = ^(NSNotification *__unused n) {
        dispatch_async(g_queue, ^{
            @autoreleasepool {
                // 録画前に失われたら失敗。録画中は delegate が結果を決める。
                if (FandheMacVCIsCurrent(rid) &&
                    (g_phase == FandheMacVCPhasePreparing || g_phase == FandheMacVCPhasePreviewing)) {
                    FandheMacVCFinishKind(rid, 9);
                }
            }
        });
    };
    NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
    [g_tokens addObject:[nc addObserverForName:AVCaptureSessionRuntimeErrorNotification
                                        object:session
                                         queue:nil
                                    usingBlock:onFailure]];
    [g_tokens addObject:[nc addObserverForName:AVCaptureDeviceWasDisconnectedNotification
                                        object:device
                                         queue:nil
                                    usingBlock:onFailure]];
    FandheMacVCController *ctl = [FandheMacVCController new];
    ctl.requestId = rid;
    g_ctl = ctl;
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            FandheMacVCPresent(rid, ctl, session);
        }
    });
}

int32_t fandhe_macos_video_capture_start(uint64_t request_id, uint32_t max_duration_secs) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        g_queue = dispatch_queue_create("com.fandhe.macos.video-capture", DISPATCH_QUEUE_SERIAL);
    });
    if (g_queue == nil) {
        return 1;
    }
    dispatch_async(g_queue, ^{
        @autoreleasepool {
            if (g_active) {
                // 2 回目の要求。既存のパネルには触らない。
                fandhe_rust_video_capture_complete(request_id, 9, 0, 0, 0, 0, 0);
                return;
            }
            if (!FandheMacVCRetryLeftover()) {
                fandhe_rust_video_capture_complete(request_id, 9, 0, 0, 0, 0, 0);
                return;
            }
            g_active = YES;
            g_rid = request_id;
            g_phase = FandheMacVCPhasePreparing;
            g_cancelled = NO;
            // 許可は毎回 OS に問う（応答待ちの間に状態が変わっていないかは応答後に再確認する）。
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo
                                     completionHandler:^(BOOL granted) {
                                       dispatch_async(g_queue, ^{
                                         @autoreleasepool {
                                             if (!FandheMacVCIsCurrent(request_id) ||
                                                 g_phase != FandheMacVCPhasePreparing) {
                                                 return;
                                             }
                                             if (!granted) {
                                                 FandheMacVCFinishKind(request_id, 3);
                                                 return;
                                             }
                                             FandheMacVCPrepare(request_id, max_duration_secs);
                                         }
                                       });
                                     }];
        }
    });
    return 0;
}
