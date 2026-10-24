// iOS 向け書類スキャン（VisionKit `VNDocumentCameraViewController`、Issue #748、親 #145）。
// `src/device/ios_document_scan.rs` から呼ばれ、`build.rs` が feature `device` 有効時にコンパイルして
// VisionKit をリンクする（AVFoundation は camera 用にリンク済み）。新しい権限キーは足さない
// （`NSCameraUsageDescription` の文言に機能名を足すだけ）。
//
// C ABI（Rust 側の `// SAFETY:` の根拠）:
// - `int32_t fandhe_ios_document_scan_start(uint64_t request_id)`
//   戻り値 0 = 受理（結果は後で `fandhe_rust_document_scan_complete` から 1 回だけ届く）/ それ以外 = 内部エラー。
// - Rust 側の受け口:
//   `int32_t fandhe_rust_document_scan_try_begin_present(uint64_t request_id)`（1 = 表示してよい。0 なら
//   表示も完了の送信もしない）、
//   `void fandhe_rust_document_scan_complete(uint64_t request_id, int32_t code, const uint8_t *bytes,
//   size_t len, uint32_t page_count)`（1 要求につき 1 回。`bytes` は呼び出しの間だけ有効で、Rust が戻る前に
//   コピーする。直後に Rust が再描画を要求する）。
//
// 完了コード（Rust 側 `outcome_from_ios` と一致させる）:
// 0 = スキャンした（bytes・page_count あり）/ 1 = 利用者が閉じた / 2 = 非対応（isSupported が NO）/
// 3 = カメラ権限の拒否・制限 / 4 = 0 ページ / 5 = 前面でない / 6 = 表示前の中断 /
// 7 = 内部エラー（didFailWithError・表示先なし・提示失敗・JPEG 変換の失敗）。0 以外では bytes を付けない。
//
// 並行性: UI 操作と done フラグ・強参照は main thread だけが触る。main への dispatch_sync は使わない。
// JPEG 変換は撮影 UI を閉じた後に global queue で行い、Rust の受け口もその queue から呼ぶ。
// 画像は長辺 2048 画素以下に縮めてから JPEG（品質 0.8）にする（親 #145 の実装方針。
// もともと 2048 以下なら描き直さない。Rust 側では縮めないため、縮小はこの層の義務）。
// 提示の完了コールバックが猶予（5 秒）を過ぎても来ない場合は code 7 で確定し、保留と強参照を残さない
// （この失敗だけは撮影 UI が出ていない前提で先に返す。遅れて出た UI は提示の completion が閉じる）。
// 結果は撮影 UI が閉じたのを観測した後にだけ送る。遷移中は UIKit の遷移コーディネータの完了を待って
// dismiss をやり直す（タイマーによる推測はしない）。
// 画像の中身・NSError の文言はログに出さない。
// 守らない範囲: 撮影 UI の表示中に Rust 側から閉じること、OS ごとの補正結果の違い、複数プロセス、
// プロセス死後の結果。

#import <AVFoundation/AVFoundation.h>
#import <UIKit/UIKit.h>
#import <VisionKit/VisionKit.h>
#import "fandhe_ios_common.h"

extern int32_t fandhe_rust_document_scan_try_begin_present(uint64_t request_id);
extern void fandhe_rust_document_scan_complete(uint64_t request_id, int32_t code, const uint8_t *bytes,
                                               size_t len, uint32_t page_count);

// 縮小後の長辺の上限（画素）。
static const CGFloat kFandheDocScanMaxLongSide = 2048.0;
// 提示の完了コールバックを待つ猶予（秒）。
static const int64_t kFandheDocScanPresentGraceSec = 5;

// 長辺が上限を超えるときだけ描き直す。scale = 1 で画素数をそのままポイント数として扱う。
static UIImage *FandheDocScanDownscaled(UIImage *image) {
    CGFloat w = image.size.width * image.scale;
    CGFloat h = image.size.height * image.scale;
    CGFloat longSide = MAX(w, h);
    if (longSide <= kFandheDocScanMaxLongSide || longSide <= 0) {
        return image;
    }
    CGFloat ratio = kFandheDocScanMaxLongSide / longSide;
    CGSize target = CGSizeMake(MAX(1.0, floor(w * ratio)), MAX(1.0, floor(h * ratio)));
    UIGraphicsImageRendererFormat *format = [UIGraphicsImageRendererFormat defaultFormat];
    format.scale = 1;
    UIGraphicsImageRenderer *renderer = [[UIGraphicsImageRenderer alloc] initWithSize:target format:format];
    return [renderer imageWithActions:^(UIGraphicsImageRendererContext *__unused ctx) {
      [image drawInRect:CGRectMake(0, 0, target.width, target.height)];
    }];
}

static void FandheDocScanFinishCode(uint64_t request_id, int32_t code) {
    fandhe_rust_document_scan_complete(request_id, code, NULL, 0, 0);
}

@interface FandheDocumentScanDelegate : NSObject <VNDocumentCameraViewControllerDelegate>
@property(nonatomic) uint64_t requestId;
@property(nonatomic) BOOL done;
// dismiss を開始済みか。isBeingDismissed は遷移キューの後ろでは NO のままなので、自前の旗で排他にする。
@property(nonatomic) BOOL dismissStarted;
- (BOOL)claim;
@end

// delegate は weak のため、完了まで強参照で保持する（main thread のみ）。
static FandheDocumentScanDelegate *g_retainedDelegate = nil;

@implementation FandheDocumentScanDelegate
// 完了を 1 回に絞り、強参照を手放す。YES なら呼び出し側が結果を送る。
- (BOOL)claim {
    if (self.done) {
        return NO;
    }
    self.done = YES;
    if (g_retainedDelegate == self) {
        g_retainedDelegate = nil;
    }
    return YES;
}
// 撮影 UI が閉じたのを見てから block を実行する（main thread）。
// 遷移中（present / dismiss のアニメーション中）は dismiss が捨てられるため、遷移の完了を待ってやり直す。
// 提示が外れていれば即実行し、そうでなければ dismiss を 1 回だけ始めてその completion で実行する。
// block を持つのは claim に勝った 1 回だけで、dismissStarted 後の呼び出しは空 block の後片付けのみ。
- (void)closeController:(VNDocumentCameraViewController *)controller then:(void (^)(void))block {
    id<UIViewControllerTransitionCoordinator> coordinator = controller.transitionCoordinator;
    if (coordinator != nil) {
        [coordinator animateAlongsideTransition:nil
                                     completion:^(id<UIViewControllerTransitionCoordinatorContext> __unused ctx) {
                                       [self closeController:controller then:block];
                                     }];
        return;
    }
    if (self.dismissStarted || controller.presentingViewController == nil) {
        block();
        return;
    }
    self.dismissStarted = YES;
    [controller dismissViewControllerAnimated:YES completion:block];
}
- (void)documentCameraViewController:(VNDocumentCameraViewController *)controller
                   didFinishWithScan:(VNDocumentCameraScan *)scan {
    if (![self claim]) {
        // 確定済みでも撮影 UI は必ず閉じる（利用者が抜けられなくなるのを防ぐ）。
        [self closeController:controller then:^{}];
        return;
    }
    uint64_t rid = self.requestId;
    NSUInteger pageCount = scan.pageCount;
    // pageCount を確かめてから呼ぶ（範囲外は例外になる）。画像は main で取り出し、変換は別 queue。
    UIImage *image = pageCount > 0 ? [scan imageOfPageAtIndex:0] : nil;
    [self closeController:controller
                     then:^{
                       if (pageCount == 0) {
                           FandheDocScanFinishCode(rid, 4);
                           return;
                       }
                       dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                         @autoreleasepool {
                             UIImage *scaled = image ? FandheDocScanDownscaled(image) : nil;
                             NSData *jpeg = scaled ? UIImageJPEGRepresentation(scaled, 0.8) : nil;
                             if (jpeg == nil || jpeg.length == 0) {
                                 FandheDocScanFinishCode(rid, 7);
                                 return;
                             }
                             uint32_t pages = pageCount > UINT32_MAX ? UINT32_MAX : (uint32_t)pageCount;
                             fandhe_rust_document_scan_complete(rid, 0, (const uint8_t *)jpeg.bytes,
                                                                jpeg.length, pages);
                         }
                       });
                     }];
}
- (void)documentCameraViewControllerDidCancel:(VNDocumentCameraViewController *)controller {
    if (![self claim]) {
        // 確定済みでも撮影 UI は必ず閉じる（利用者が抜けられなくなるのを防ぐ）。
        [self closeController:controller then:^{}];
        return;
    }
    uint64_t rid = self.requestId;
    [self closeController:controller then:^{ FandheDocScanFinishCode(rid, 1); }];
}
- (void)documentCameraViewController:(VNDocumentCameraViewController *)controller
                    didFailWithError:(NSError *)error {
    if (![self claim]) {
        // 確定済みでも撮影 UI は必ず閉じる（利用者が抜けられなくなるのを防ぐ）。
        [self closeController:controller then:^{}];
        return;
    }
    uint64_t rid = self.requestId;
    [self closeController:controller then:^{ FandheDocScanFinishCode(rid, 7); }];
}
@end

// 表示先として使えるなら最前面の VC を返す。使えなければ nil。
static UIViewController *FandheDocScanPresenter(void) {
    UIViewController *top = FandheTopmostViewController();
    if (top == nil || top.view.window == nil || top.presentedViewController != nil ||
        top.isBeingPresented || top.isBeingDismissed) {
        return nil;
    }
    return top;
}

// 撮影 UI を提示する。try_begin_present 済みの要求だけがここへ来る（main thread）。
static void FandheDocScanPresent(uint64_t request_id) {
    UIViewController *top = FandheDocScanPresenter();
    if (top == nil) {
        FandheDocScanFinishCode(request_id, 7);
        return;
    }
    FandheDocumentScanDelegate *delegate = [FandheDocumentScanDelegate new];
    delegate.requestId = request_id;
    VNDocumentCameraViewController *vc = [[VNDocumentCameraViewController alloc] init];
    vc.delegate = delegate;
    // delegate を通らない閉じ方（スワイプ）を塞ぐ。
    vc.modalPresentationStyle = UIModalPresentationFullScreen;
    vc.modalInPresentation = YES;
    g_retainedDelegate = delegate;
    // 提示の完了を示す旗（main thread のみ）。猶予タイマーが参照する。
    __block BOOL presented = NO;
    // 提示は非同期に確定するため、成否は completion 内で判定する。
    [top presentViewController:vc
                      animated:YES
                    completion:^{
                      presented = YES;
                      // 確定済み（猶予切れ・失敗を含む）でも、画面に出た撮影 UI は閉じる。
                      // closeController は冪等で、遷移を待つ判断もそこだけが持つ。
                      if (delegate.done) {
                          [delegate closeController:vc then:^{}];
                          return;
                      }
                      if (top.presentedViewController != vc) {
                          if ([delegate claim]) {
                              FandheDocScanFinishCode(request_id, 7);
                          }
                      }
                    }];
    // completion が来ないと保留と強参照が残り続ける。猶予内に提示が完了しなければ Internal で確定する。
    // 競合しても `claim` が 1 回に絞る。提示が完了していれば何もしない。
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, kFandheDocScanPresentGraceSec * NSEC_PER_SEC),
                   dispatch_get_main_queue(), ^{
                     if (presented || ![delegate claim]) {
                         return;
                     }
                     FandheDocScanFinishCode(request_id, 7);
                     // 失敗は先に返す（Rust の期限は表示開始までで、提示の完了が来ない場合の唯一の出口）。
                     // 遅れて表示された場合は、提示の completion が closeController で閉じる。
                     [delegate closeController:vc then:^{}];
                   });
}

int32_t fandhe_ios_document_scan_start(uint64_t request_id) {
    // egui のフレーム中に UIKit の遷移を始めないため main queue へ積む。
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (!VNDocumentCameraViewController.isSupported) {
                FandheDocScanFinishCode(request_id, 2);
                return;
            }
            if (UIApplication.sharedApplication.applicationState == UIApplicationStateBackground) {
                FandheDocScanFinishCode(request_id, 5);
                return;
            }
            AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo];
            if (status == AVAuthorizationStatusDenied || status == AVAuthorizationStatusRestricted) {
                FandheDocScanFinishCode(request_id, 3);
                return;
            }
            if (FandheDocScanPresenter() == nil) {
                FandheDocScanFinishCode(request_id, 7);
                return;
            }
            // 期限は権限ダイアログを含む最初の UI が出るまで。0 なら表示も完了送信もしない。
            if (fandhe_rust_document_scan_try_begin_present(request_id) == 0) {
                return;
            }
            if (status == AVAuthorizationStatusNotDetermined) {
                [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo
                                         completionHandler:^(BOOL granted) {
                                           dispatch_async(dispatch_get_main_queue(), ^{
                                             @autoreleasepool {
                                                 if (!granted) {
                                                     FandheDocScanFinishCode(request_id, 3);
                                                     return;
                                                 }
                                                 // Inactive はダイアログ由来で起きるため中断扱いにしない。
                                                 if (UIApplication.sharedApplication.applicationState ==
                                                     UIApplicationStateBackground) {
                                                     FandheDocScanFinishCode(request_id, 6);
                                                     return;
                                                 }
                                                 FandheDocScanPresent(request_id);
                                             }
                                           });
                                         }];
                return;
            }
            FandheDocScanPresent(request_id);
        }
    });
    return 0;
}
