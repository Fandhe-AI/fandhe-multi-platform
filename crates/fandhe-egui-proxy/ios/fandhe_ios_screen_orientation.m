// 画面向きの取得・固定: iOS 実装（Issue #656、親 #128、#655 で dispatch・trait 確定済み）。
//
// `src/device/ios_screen_orientation.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（UIKit / objc ランタイムはリンク済み）。
//
// 前提とする winit は 0.30.13（`WinitUIViewController` は `platform_impl/ios/view_controller.rs`）。
// 版が変わってクラス名や構造が変わると差し込めず、request / read は 3 を返す（嘘の成功にしない）。
//
// 方式: eframe は winit の `Window` を外へ出さないため `WindowExtIOS::set_valid_orientations` は
// 使えない。代わりに `WinitUIViewController` の `supportedInterfaceOrientations` を
// `class_replaceMethod` で差し替え、希望が縦なら Portrait、横なら Landscape のマスクを返す。
// 解除（希望なし）では元の IMP を呼び継ぐ。型エンコードは元メソッドから取る。前例は
// `fandhe_ios_fullscreen_immersive.m`（別セレクタなので衝突しない）。
//
// 契約:
//   fandhe_ios_screen_orientation_can_lock(void) -> int32_t
//     1 = 固定できる（main スレッドかつ iPhone idiom かつ Mac 上の iOS アプリでない）、0 = できない。
//   fandhe_ios_screen_orientation_request(int32_t lock_code) -> int32_t
//     lock_code: 0 = 解除、1 = 縦、2 = 横。
//     0 = 依頼を出した。2 = main 以外（UIKit に触れず返す）。
//     3 = root が WinitUIViewController でない / クラスが無い / windowScene が nil / 差し込み失敗。
//     4 = 固定できない端末。5 = 未知の lock_code。
//     失敗経路はすべて希望値を変える前に返す（`Err` なら OS 側の希望は変えない契約）。
//   fandhe_ios_screen_orientation_read(int32_t *orientation, int32_t *degrees, int32_t *lock) -> int32_t
//     0 = 読めた、2 = main 以外（out 引数に書かない）、3 = 読めない（Unknown を含む）。
//     orientation: 1 = 縦・2 = 横。degrees: Portrait 0 / LandscapeRight 90 / PortraitUpsideDown 180 /
//     LandscapeLeft 270（Android の ROTATION_90・Web の landscape-primary と揃える）。
//     lock: 希望値のコード（0/1/2）。
//
// requestGeometryUpdate はシーンの回転を要求するだけで、固定自体は supportedInterfaceOrientations の
// 差し替えと setNeedsUpdateOfSupportedInterfaceOrientations で適用済み。errorHandler は希望値を
// 戻さない（すでにその向きのシーンへの更新は UISceneErrorDomain 101 で拒否されうるため、戻すと
// 有効な固定を黙って失う。復元要求の失敗が期限切れの固定を復活させる逆の問題も同じ根）。
// 回転できなかった場合は読み戻しの向きが依頼と食い違うので、driver は期限で request_lock(直前値) の
// 復元へ進む。現世代の失敗なら `fandhe_rust_screen_orientation_geometry_failed` で次フレームの
// 読み戻しを起こす。古い世代の失敗は捨てる。
// NSError の内容はログに出さない（security.md A09）。
//
// スレッド: static は main スレッドでのみ読み書きする。main 以外は `dispatch_sync` で main を
// 待たず（デッドロック回避、`device-bridge-contracts.md`「11」）2 を返す。全関数は @autoreleasepool 内。
//
// 守る範囲: 1 プロセス内で読み戻しが実際の向きと希望に追従、固定できていない状態を固定中と見せない。
// 守らない範囲: 回転アニメーション中の遅れ、背景遷移中・root が winit の VC でない間の状態
// （3 を返す）、iPad・Mac 上の iOS アプリの固定、マルチウィンドウでの OS による無視、終了時の後片付け。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <objc/runtime.h>

#import "fandhe_ios_common.h"

// Rust 側（`ios_screen_orientation.rs`）が定義する。errorHandler の直後に再描画を起こす。
extern void fandhe_rust_screen_orientation_geometry_failed(void);

// 希望値（0 = なし、1 = 縦、2 = 横）。main スレッドでのみ読み書きする。
static int32_t fandhe_orientation_desired = 0;
static uint64_t fandhe_orientation_generation = 0;
static BOOL fandhe_orientation_installed = NO;
static IMP fandhe_orig_supported_orientations = NULL;

static UIInterfaceOrientationMask fandhe_supported_orientations(id self, SEL _cmd) {
    if (fandhe_orientation_desired == 1) {
        return UIInterfaceOrientationMaskPortrait;
    }
    if (fandhe_orientation_desired == 2) {
        return UIInterfaceOrientationMaskLandscape;
    }
    if (fandhe_orig_supported_orientations != NULL) {
        return ((UIInterfaceOrientationMask(*)(id, SEL))fandhe_orig_supported_orientations)(self, _cmd);
    }
    return UIInterfaceOrientationMaskAllButUpsideDown;
}

// 差し込みを 1 回だけ行う。クラス・元メソッドが無ければ NO（何も変更しない）。
static BOOL fandhe_orientation_install(void) {
    if (fandhe_orientation_installed) {
        return YES;
    }
    Class cls = objc_getClass("WinitUIViewController");
    if (cls == Nil) {
        return NO;
    }
    Method m = class_getInstanceMethod(cls, @selector(supportedInterfaceOrientations));
    if (m == NULL) {
        return NO;
    }
    // 継承したメソッドだと class_replaceMethod は NULL を返すため、親の実装を先に取っておく。
    IMP inherited = method_getImplementation(m);
    IMP replaced = class_replaceMethod(
        cls, @selector(supportedInterfaceOrientations), (IMP)fandhe_supported_orientations,
        method_getTypeEncoding(m));
    fandhe_orig_supported_orientations = replaced != NULL ? replaced : inherited;
    fandhe_orientation_installed = YES;
    return YES;
}

static BOOL fandhe_orientation_can_lock_now(void) {
    return UIDevice.currentDevice.userInterfaceIdiom == UIUserInterfaceIdiomPhone &&
           !NSProcessInfo.processInfo.isiOSAppOnMac;
}

static UIViewController *_Nullable fandhe_orientation_winit_vc(void) {
    UIViewController *vc = FandheRootViewController();
    Class cls = objc_getClass("WinitUIViewController");
    if (vc == nil || cls == Nil || ![vc isKindOfClass:cls]) {
        return nil;
    }
    return vc;
}

int32_t fandhe_ios_screen_orientation_can_lock(void) {
    if (![NSThread isMainThread]) {
        return 0;
    }
    @autoreleasepool {
        return fandhe_orientation_can_lock_now() ? 1 : 0;
    }
}

int32_t fandhe_ios_screen_orientation_request(int32_t lock_code) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        if (lock_code < 0 || lock_code > 2) {
            return 5;
        }
        if (!fandhe_orientation_can_lock_now()) {
            return 4;
        }
        UIViewController *vc = fandhe_orientation_winit_vc();
        UIWindowScene *scene = vc.view.window.windowScene;
        if (vc == nil || scene == nil || !fandhe_orientation_install()) {
            return 3;
        }
        // 失敗経路は上で返し済み。ここから先は希望値を変えてよい。
        fandhe_orientation_desired = lock_code;
        uint64_t generation = ++fandhe_orientation_generation;
        [vc setNeedsUpdateOfSupportedInterfaceOrientations];
        if (lock_code != 0) {
            UIInterfaceOrientationMask mask = (lock_code == 1)
                                                  ? UIInterfaceOrientationMaskPortrait
                                                  : UIInterfaceOrientationMaskLandscape;
            UIWindowSceneGeometryPreferencesIOS *prefs =
                [[UIWindowSceneGeometryPreferencesIOS alloc] initWithInterfaceOrientations:mask];
            [scene requestGeometryUpdateWithPreferences:prefs
                                           errorHandler:^(NSError *_Nonnull error) {
                                               (void)error;
                                               dispatch_async(dispatch_get_main_queue(), ^{
                                                   // 古い世代の失敗は捨てる（契約表 2）。希望値は触らない。
                                                   if (generation == fandhe_orientation_generation) {
                                                       fandhe_rust_screen_orientation_geometry_failed();
                                                   }
                                               });
                                           }];
        }
    }
    return 0;
}

int32_t fandhe_ios_screen_orientation_read(int32_t *orientation, int32_t *degrees, int32_t *lock) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        UIViewController *vc = fandhe_orientation_winit_vc();
        UIWindowScene *scene = vc.view.window.windowScene;
        if (vc == nil || scene == nil) {
            return 3;
        }
        int32_t o, d;
        switch (scene.effectiveGeometry.interfaceOrientation) {
        case UIInterfaceOrientationPortrait:
            o = 1;
            d = 0;
            break;
        case UIInterfaceOrientationLandscapeRight:
            o = 2;
            d = 90;
            break;
        case UIInterfaceOrientationPortraitUpsideDown:
            o = 1;
            d = 180;
            break;
        case UIInterfaceOrientationLandscapeLeft:
            o = 2;
            d = 270;
            break;
        default:
            return 3;
        }
        *orientation = o;
        *degrees = d;
        *lock = fandhe_orientation_desired;
    }
    return 0;
}
