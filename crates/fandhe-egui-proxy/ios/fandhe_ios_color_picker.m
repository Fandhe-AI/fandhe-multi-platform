// Phase 11 の拡充ループ（親 #184）: システムの色選択ダイアログ iOS 実装（Issue #942）。
// `UIColorPickerViewController`（supportsAlpha = NO）を固定の初期色で提示し、閉じた時点の色を
// sRGB に直して 0xRRGGBB の整数で Rust へ通知する。
//
// `crates/fandhe-egui-proxy/src/device/ios_color_picker.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `compile_ios_color_picker()` が iOS 分岐・feature `device` 有効時にのみコンパイルする
// （UIKit は既存のリンクで足りる）。
//
// 契約（pull 型。`macos/fandhe_macos_color_picker.m` と同型）:
//   int32_t fandhe_ios_color_picker_present(uint64_t request_id, int32_t initial_rgb)
//     main queue へ dispatch してすぐ 0 を返す。提示の失敗も含め結果は非同期に下の関数で届く。
//   fandhe_rust_ios_color_picker_outcome(uint64_t request_id, int32_t code, int32_t rgb)
//     -> Rust 側（`ios_color_picker.rs`）が公開する `extern "C"` 関数。1 要求につき 1 回だけ呼ぶ。
//        code 表: 0 = 選択（rgb = 0xRRGGBB）、1 = 色を選ばずに閉じた、2 = sRGB 変換の失敗・成分が NaN、
//        3 = 提示できなかった（背景・提示先なし）。code → 結果の写像の SSOT は Rust の
//        `fandhe_device::color_picker::result_from_apple_code`（`.m` では写像しない）。
//
// 確定: 確定ボタンは無い。閉じる（colorPickerViewControllerDidFinish: / スワイプ閉じの
// presentationControllerDidDismiss:）までに didSelectColor が 1 回以上来ていれば、閉じた時点の
// selectedColor を結果とする。来ていなければ「選ばずに閉じた」。初期色との比較では推測しない。
// 初期色はプログラムで入れてから delegate を付ける（プログラムの設定を「選んだ」と取り違えない）。
//
// スレッド: 生成・提示・通知はすべて main queue。完了通知は 1 要求につき 1 回（`done`、first-wins）。
// 通知前に状態を確定し、delegate の強参照を外す。
//
// 守らない範囲: 表示中の背景遷移、閉じた通知が届かない場合の復旧、sRGB 外の色の正確さ、
// iPad での提示形態（既定の sheet のまま）。
//
// 情報非保持: 選んだ色はフィンガープリントに使えるため、`NSLog` を含むログ出力は一切行わない。
// ObjC の例外を Rust へ通さないため、例外を投げる API は使わない。

#import <UIKit/UIKit.h>
#include <math.h>
#include <stdint.h>

#import "fandhe_ios_common.h"

extern void fandhe_rust_ios_color_picker_outcome(uint64_t request_id, int32_t code, int32_t rgb);

// sRGB の外の色は各成分を 0〜1 に寄せてから丸める。NaN は -1。
static int32_t channel(CGFloat c) {
    if (isnan(c)) {
        return -1;
    }
    if (c < 0.0) c = 0.0;
    if (c > 1.0) c = 1.0;
    return (int32_t)lround(c * 255.0);
}

@interface FandheColorPickerDelegate : NSObject <UIColorPickerViewControllerDelegate, UIAdaptivePresentationControllerDelegate>
@property(nonatomic) uint64_t requestId;
@property(nonatomic) BOOL changed;
@property(nonatomic) BOOL done;
- (void)finishWithController:(UIColorPickerViewController *)vc;
- (void)finishWithCode:(int32_t)code rgb:(int32_t)rgb;
@end

// delegate は weak 参照のため、完了まで強参照で保持する（完了時に外す）。
static FandheColorPickerDelegate *g_retainedDelegate = nil;

@implementation FandheColorPickerDelegate

- (void)colorPickerViewController:(UIColorPickerViewController *)viewController
                   didSelectColor:(UIColor *)color
                     continuously:(BOOL)continuously {
    self.changed = YES;
}

- (void)colorPickerViewControllerDidFinish:(UIColorPickerViewController *)viewController {
    [self finishWithController:viewController];
}

- (void)presentationControllerDidDismiss:(UIPresentationController *)presentationController {
    UIViewController *vc = presentationController.presentedViewController;
    [self finishWithController:[vc isKindOfClass:UIColorPickerViewController.class]
                                   ? (UIColorPickerViewController *)vc
                                   : nil];
}

- (void)finishWithController:(UIColorPickerViewController *)vc {
    if (self.done) {
        return;
    }
    if (!self.changed) {
        [self finishWithCode:1 rgb:0];
        return;
    }
    CGFloat r = 0, g = 0, b = 0, a = 0;
    if (vc == nil || ![vc.selectedColor getRed:&r green:&g blue:&b alpha:&a]) {
        [self finishWithCode:2 rgb:0];
        return;
    }
    int32_t ri = channel(r), gi = channel(g), bi = channel(b);
    if (ri < 0 || gi < 0 || bi < 0) {
        [self finishWithCode:2 rgb:0];
    } else {
        [self finishWithCode:0 rgb:(ri << 16) | (gi << 8) | bi];
    }
}

- (void)finishWithCode:(int32_t)code rgb:(int32_t)rgb {
    if (self.done) {
        return;
    }
    uint64_t rid = self.requestId;
    self.done = YES;
    // 後続の要求が保持枠を引き継いでいる場合は、その強参照を外さない。
    if (g_retainedDelegate == self) {
        g_retainedDelegate = nil;
    }
    // 強参照を外した後は self に触れない。
    fandhe_rust_ios_color_picker_outcome(rid, code, rgb);
}

@end

int32_t fandhe_ios_color_picker_present(uint64_t request_id, int32_t initial_rgb) {
    // egui のフレーム中に UIKit の遷移を始めないため main queue へ積む。
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (UIApplication.sharedApplication.applicationState == UIApplicationStateBackground) {
                fandhe_rust_ios_color_picker_outcome(request_id, 3, 0);
                return;
            }
            UIViewController *top = FandheTopmostViewController();
            if (top == nil || top.view.window == nil || top.presentedViewController != nil ||
                top.isBeingPresented || top.isBeingDismissed) {
                fandhe_rust_ios_color_picker_outcome(request_id, 3, 0);
                return;
            }
            UIColorPickerViewController *vc = [UIColorPickerViewController new];
            vc.supportsAlpha = NO;
            // 初期色を先に入れる。delegate はその後。
            vc.selectedColor = [UIColor colorWithRed:((initial_rgb >> 16) & 0xFF) / 255.0
                                               green:((initial_rgb >> 8) & 0xFF) / 255.0
                                                blue:(initial_rgb & 0xFF) / 255.0
                                               alpha:1.0];
            FandheColorPickerDelegate *delegate = [FandheColorPickerDelegate new];
            delegate.requestId = request_id;
            vc.delegate = delegate;
            vc.presentationController.delegate = delegate;
            // 前の要求が未通知のまま枠を上書きされないよう、先に「選ばずに閉じた」で確定する。
            [g_retainedDelegate finishWithCode:1 rgb:0];
            g_retainedDelegate = delegate;
            // 提示は非同期に確定するため、成否は completion 内で判定する。
            [top presentViewController:vc
                              animated:YES
                            completion:^{
                              if (!delegate.done && top.presentedViewController != vc) {
                                  [delegate finishWithCode:3 rgb:0];
                              }
                            }];
        }
    });
    return 0;
}
