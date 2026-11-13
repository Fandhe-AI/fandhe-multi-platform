// iOS 向け動画の撮影（`UIImagePickerController` の動画モード、Issue #822、親 #169）。
// `src/device/ios_video_capture.rs` から呼ばれ、`build.rs` が feature `device` 有効時にコンパイルする
// （AVFoundation・UIKit・UniformTypeIdentifiers は既存のリンクで足り、CoreMedia だけ明示リンクする）。
// 新しい権限キーは足さない（`NSCameraUsageDescription`・`NSMicrophoneUsageDescription` の文言に機能名を足すだけ）。
// マイク許可は事前に要求しない（撮影 UI が必要に応じて OS の確認を出す）。
//
// C ABI（Rust 側の `// SAFETY:` の根拠）:
// - `int32_t fandhe_ios_video_capture_start(uint64_t request_id, uint32_t max_duration_secs)`
//   戻り値 0 = 受理（結果は後で `fandhe_rust_video_capture_complete` から 1 回だけ届く）/ それ以外 = 内部エラー。
// - Rust 側の受け口:
//   `int32_t fandhe_rust_video_capture_try_begin_present(uint64_t request_id)`（1 = 表示してよい。0 なら
//   表示も完了の送信もしない）、
//   `void fandhe_rust_video_capture_complete(uint64_t request_id, int32_t kind, uint64_t size_bytes,
//   int64_t duration_ms, int32_t width, int32_t height, int32_t rotation_deg)`（1 要求につき 1 回。
//   直後に Rust が再描画を要求する）。ポインタは渡さない。パス・NSError の文言は Rust へ渡さない。
//
// kind（`fandhe_device::video_capture::result_from_code` の表と一致させる）:
// 0 = 撮れた / 1 = 利用者が閉じた / 2 = 非対応（カメラなし・動画モードなし。シミュレータもここ）/
// 3 = カメラ権限の拒否・制限 / 4 = 撮影アプリなし（iOS では使わない）/ 5 = 前面でない /
// 6 = 表示前の中断 / 7 = 0 バイト・動画なし / 8 = メタデータ（映像トラック・寸法）を読めない / 9 = その他。
// kind 0 以外ではメタデータ引数は無視される（0 を渡す）。duration_ms = -1 は「長さなし」（失敗にしない）。
// rotation_deg は preferredTransform から求めた 0..360 の整数（90 の倍数でなければ Rust が Unreadable にする）。
//
// 並行性: UI 操作と done フラグ・強参照は main thread だけが触る。main への dispatch_sync は使わない。
// メタデータの読み取りは撮影 UI を閉じた後に global queue で行い、Rust の受け口もその queue から呼ぶ。
// 結果は撮影 UI が閉じたのを観測した後にだけ送る。遷移中は UIKit の遷移コーディネータの完了を待って
// dismiss をやり直す（タイマーによる推測はしない）。
// 期限・猶予タイマーは持たない（完了は OS の delegate 通知だけで決める。長さの上限は
// `videoMaximumDuration` で OS に任せる）。
// 一時ファイル（`UIImagePickerControllerMediaURL`）は成功・取り消し・失敗・claim 負けの全経路で削除する。
// 動画の中身・パス・NSError の文言はログに出さない。
// 守らない範囲: 提示の completion が届かない場合の救済、録画中の背景遷移の扱い（OS 任せ）、
// 撮影 UI の表示中に Rust 側から閉じること、複数プロセス、プロセス死後の結果。

#import <AVFoundation/AVFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <UIKit/UIKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#import "fandhe_ios_common.h"

extern int32_t fandhe_rust_video_capture_try_begin_present(uint64_t request_id);
extern void fandhe_rust_video_capture_complete(uint64_t request_id, int32_t kind, uint64_t size_bytes,
                                               int64_t duration_ms, int32_t width, int32_t height,
                                               int32_t rotation_deg);

// 一時ファイルを消してから Rust へ結果を渡す唯一の出口（全経路がここを通る）。
static void FandheVideoFinish(uint64_t rid, int32_t kind, uint64_t size, int64_t dur, int32_t w, int32_t h,
                              int32_t rot, NSURL *_Nullable url) {
    if (url != nil) {
        [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
    }
    fandhe_rust_video_capture_complete(rid, kind, size, dur, w, h, rot);
}

static void FandheVideoFinishKind(uint64_t rid, int32_t kind, NSURL *_Nullable url) {
    FandheVideoFinish(rid, kind, 0, 0, 0, 0, 0, url);
}

// 映像トラックの寸法と回転を読み、一時ファイルを消して結果を送る（global queue 上の completion）。
static void FandheVideoReadTrack(uint64_t rid, NSURL *url, uint64_t size, int64_t durMs, AVAssetTrack *track) {
    [track loadValuesAsynchronouslyForKeys:@[ @"naturalSize", @"preferredTransform" ]
                         completionHandler:^{
                           @autoreleasepool {
                               NSError *e1 = nil;
                               NSError *e2 = nil;
                               if ([track statusOfValueForKey:@"naturalSize" error:&e1] != AVKeyValueStatusLoaded ||
                                   [track statusOfValueForKey:@"preferredTransform" error:&e2] !=
                                       AVKeyValueStatusLoaded) {
                                   FandheVideoFinishKind(rid, 8, url);
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
                               FandheVideoFinish(rid, 0, size, durMs, (int32_t)lround(natural.width),
                                                 (int32_t)lround(natural.height), (int32_t)r, url);
                           }
                         }];
}

// 撮影後の動画からサイズ・長さ・寸法・回転を読み、一時ファイルを消して結果を送る（global queue）。
static void FandheVideoReadAndFinish(uint64_t rid, NSURL *url) {
    NSNumber *fileSize = nil;
    [url getResourceValue:&fileSize forKey:NSURLFileSizeKey error:nil];
    uint64_t size = fileSize != nil ? fileSize.unsignedLongLongValue : 0;
    if (size == 0) {
        FandheVideoFinishKind(rid, 7, url);
        return;
    }
    AVURLAsset *asset = [AVURLAsset URLAssetWithURL:url options:nil];
    [asset loadValuesAsynchronouslyForKeys:@[ @"duration" ]
                         completionHandler:^{
                           @autoreleasepool {
                               int64_t durMs = -1;
                               NSError *err = nil;
                               if ([asset statusOfValueForKey:@"duration" error:&err] == AVKeyValueStatusLoaded) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
                                   CMTime d = asset.duration;
#pragma clang diagnostic pop
                                   if (CMTIME_IS_NUMERIC(d) && d.timescale > 0 && d.value >= 0) {
                                       durMs = (int64_t)CMTimeConvertScale(d, 1000, kCMTimeRoundingMethod_Default).value;
                                   }
                               }
                               [asset loadTracksWithMediaType:AVMediaTypeVideo
                                            completionHandler:^(NSArray<AVAssetTrack *> *tracks, NSError *e) {
                                              @autoreleasepool {
                                                  AVAssetTrack *track = tracks.firstObject;
                                                  if (e != nil || track == nil) {
                                                      FandheVideoFinishKind(rid, 8, url);
                                                      return;
                                                  }
                                                  FandheVideoReadTrack(rid, url, size, durMs, track);
                                              }
                                            }];
                           }
                         }];
}

@interface FandheVideoCaptureDelegate : NSObject <UIImagePickerControllerDelegate, UINavigationControllerDelegate>
@property(nonatomic) uint64_t requestId;
@property(nonatomic) BOOL done;
// dismiss を開始済みか。isBeingDismissed は遷移キューの後ろでは NO のままなので、自前の旗で排他にする。
@property(nonatomic) BOOL dismissStarted;
- (BOOL)claim;
@end

// delegate は weak のため、完了まで強参照で保持する（main thread のみ）。
static FandheVideoCaptureDelegate *g_retainedVideoDelegate = nil;

@implementation FandheVideoCaptureDelegate
// 完了を 1 回に絞り、強参照を手放す。YES なら呼び出し側が結果を送る。
- (BOOL)claim {
    if (self.done) {
        return NO;
    }
    self.done = YES;
    if (g_retainedVideoDelegate == self) {
        g_retainedVideoDelegate = nil;
    }
    return YES;
}
// 撮影 UI が閉じたのを見てから block を実行する（main thread）。遷移中は完了を待ってやり直す。
- (void)closeController:(UIViewController *)controller then:(void (^)(void))block {
    id<UIViewControllerTransitionCoordinator> coordinator = controller.transitionCoordinator;
    if (coordinator != nil) {
        // NO のとき completion は呼ばれない（遷移がすでに終わっている等）。block を落とさず下の経路へ進む。
        // YES のときだけ completion が一度呼ばれるので、block は必ず一度だけ実行される。
        BOOL registered = [coordinator animateAlongsideTransition:nil
                                                       completion:^(id<UIViewControllerTransitionCoordinatorContext> __unused ctx) {
                                                         [self closeController:controller then:block];
                                                       }];
        if (registered) {
            return;
        }
    }
    if (self.dismissStarted || controller.presentingViewController == nil) {
        block();
        return;
    }
    self.dismissStarted = YES;
    [controller dismissViewControllerAnimated:YES completion:block];
}
- (void)imagePickerController:(UIImagePickerController *)picker
    didFinishPickingMediaWithInfo:(NSDictionary<UIImagePickerControllerInfoKey, id> *)info {
    NSURL *url = info[UIImagePickerControllerMediaURL];
    if (![self claim]) {
        // 確定済みでも撮影 UI は必ず閉じ、出来たファイルは消す。
        [self closeController:picker
                         then:^{
                           if (url != nil) {
                               [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
                           }
                         }];
        return;
    }
    uint64_t rid = self.requestId;
    [self closeController:picker
                     then:^{
                       if (url == nil) {
                           FandheVideoFinishKind(rid, 7, nil);
                           return;
                       }
                       dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                         @autoreleasepool {
                             FandheVideoReadAndFinish(rid, url);
                         }
                       });
                     }];
}
- (void)imagePickerControllerDidCancel:(UIImagePickerController *)picker {
    if (![self claim]) {
        [self closeController:picker then:^{}];
        return;
    }
    uint64_t rid = self.requestId;
    [self closeController:picker then:^{ FandheVideoFinishKind(rid, 1, nil); }];
}
@end

// 表示先として使えるなら最前面の VC を返す。使えなければ nil。
static UIViewController *FandheVideoPresenter(void) {
    UIViewController *top = FandheTopmostViewController();
    if (top == nil || top.view.window == nil || top.presentedViewController != nil ||
        top.isBeingPresented || top.isBeingDismissed) {
        return nil;
    }
    return top;
}

// 撮影 UI を提示する（main thread）。
static void FandheVideoPresent(uint64_t request_id, uint32_t max_duration_secs) {
    UIViewController *top = FandheVideoPresenter();
    if (top == nil) {
        FandheVideoFinishKind(request_id, 9, nil);
        return;
    }
    // 古い要求（差し替え済み・破棄済み）は表示も完了送信もしない。
    if (fandhe_rust_video_capture_try_begin_present(request_id) == 0) {
        return;
    }
    FandheVideoCaptureDelegate *delegate = [FandheVideoCaptureDelegate new];
    delegate.requestId = request_id;
    UIImagePickerController *vc = [[UIImagePickerController alloc] init];
    vc.sourceType = UIImagePickerControllerSourceTypeCamera;
    // mediaTypes を cameraCaptureMode より先に設定する（逆だと例外になる）。
    vc.mediaTypes = @[ UTTypeMovie.identifier ];
    vc.cameraCaptureMode = UIImagePickerControllerCameraCaptureModeVideo;
    vc.videoMaximumDuration = (NSTimeInterval)max_duration_secs;
    vc.delegate = delegate;
    // delegate を通らない閉じ方（スワイプ）を塞ぐ。
    vc.modalPresentationStyle = UIModalPresentationFullScreen;
    vc.modalInPresentation = YES;
    g_retainedVideoDelegate = delegate;
    [top presentViewController:vc
                      animated:YES
                    completion:^{
                      // 確定済みでも、画面に出た撮影 UI は閉じる（closeController は冪等）。
                      if (delegate.done) {
                          [delegate closeController:vc then:^{}];
                          return;
                      }
                      if (top.presentedViewController != vc) {
                          if ([delegate claim]) {
                              FandheVideoFinishKind(request_id, 9, nil);
                          }
                      }
                    }];
}

int32_t fandhe_ios_video_capture_start(uint64_t request_id, uint32_t max_duration_secs) {
    // egui のフレーム中に UIKit の遷移を始めないため main queue へ積む。
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (![UIImagePickerController isSourceTypeAvailable:UIImagePickerControllerSourceTypeCamera] ||
                ![[UIImagePickerController availableMediaTypesForSourceType:UIImagePickerControllerSourceTypeCamera]
                    containsObject:UTTypeMovie.identifier]) {
                FandheVideoFinishKind(request_id, 2, nil);
                return;
            }
            if (UIApplication.sharedApplication.applicationState == UIApplicationStateBackground) {
                FandheVideoFinishKind(request_id, 5, nil);
                return;
            }
            AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo];
            if (status == AVAuthorizationStatusDenied || status == AVAuthorizationStatusRestricted) {
                FandheVideoFinishKind(request_id, 3, nil);
                return;
            }
            if (status == AVAuthorizationStatusNotDetermined) {
                [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo
                                         completionHandler:^(BOOL granted) {
                                           dispatch_async(dispatch_get_main_queue(), ^{
                                             @autoreleasepool {
                                                 if (!granted) {
                                                     FandheVideoFinishKind(request_id, 3, nil);
                                                     return;
                                                 }
                                                 // Inactive はダイアログ由来で起きるため中断扱いにしない。
                                                 if (UIApplication.sharedApplication.applicationState ==
                                                     UIApplicationStateBackground) {
                                                     FandheVideoFinishKind(request_id, 6, nil);
                                                     return;
                                                 }
                                                 FandheVideoPresent(request_id, max_duration_secs);
                                             }
                                           });
                                         }];
                return;
            }
            FandheVideoPresent(request_id, max_duration_secs);
        }
    });
    return 0;
}
