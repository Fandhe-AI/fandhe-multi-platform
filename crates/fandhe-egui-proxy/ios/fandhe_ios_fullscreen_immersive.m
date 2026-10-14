// 全画面表示（システムバー非表示）: iOS 実装（Issue #1313、親 #1133）。
//
// `src/device/ios_fullscreen_immersive.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（UIKit / objc ランタイムはリンク済み）。
//
// 前提とする winit は 0.30.x（`WinitUIViewController` は `platform_impl/ios/view.rs` で定義）。
// 版が変わってクラス名や構造が変わると差し込めず、set / is_on は 3 を返す（嘘の成功にしない）。
//
// 方式: eframe は winit の `Window` を外へ出さないため `WindowExtIOS` は使えない。代わりに
// winit の `WinitUIViewController` の `prefersStatusBarHidden` / `prefersHomeIndicatorAutoHidden`
// を `class_replaceMethod` で差し替え、希望値 `fandhe_fullscreen_desired` が真なら YES を返す。
// 元の IMP も呼び継ぐので winit 側の値（既定 NO）は壊さない。前例は `fandhe_ios_text_input.m`。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_ios_fullscreen_set(int32_t on) -> int32_t
//     0 = 設定した。2 = main スレッド以外（UIKit に触れず返す）。
//     3 = key window の rootViewController が WinitUIViewController でない / クラスが無い
//     （希望値は変えない。失敗を返す経路では必ず元の希望値のまま）。
//   fandhe_ios_fullscreen_is_on(void) -> int32_t
//     1 = 全画面（VC の希望とステータスバーの実状態の両方が真）、0 = 通常（実状態も表示）、
//     2 = main 以外、3 = 読めない、または希望は通常なのに実状態が hidden（未確定）。呼び出し元は 2・3 を None に写す（false で代用しない）。
//   iPhone の横向きでは OS が要求なしでバーを隠すため、VC の希望も条件に入れて嘘の成功を防ぐ。
//
// スレッド: egui の update = main から同期で呼ばれる。main 以外は `dispatch_sync` で main を
// 待たず（デッドロック回避、`device-bridge-contracts.md`「11」）2 を返す。
//
// ホームインジケータは「自動で隠す」を OS へ希望するだけで、触ると一時的に出る。読み戻す手段が
// 無いため判定はステータスバーで行う。
//
// 守る範囲: 1 プロセス内で表示が実状態に追従、失敗・読めない状態を全画面と見せない。
// 守らない範囲: 遷移中の一時的なずれ、root が winit の VC でない間（希望は反映されず 3 を返す）、横向きでの OS 自動非表示、終了時の
// 後片付け（OS がプロセスごと片付ける）。ログは出さない（security.md A09）。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <objc/runtime.h>

#import "fandhe_ios_common.h"

// 希望値。main スレッドでのみ読み書きする。
static BOOL fandhe_fullscreen_desired = NO;
static BOOL fandhe_fullscreen_installed = NO;
static IMP fandhe_orig_status_bar_hidden = NULL;
static IMP fandhe_orig_home_indicator_hidden = NULL;

static BOOL fandhe_prefers_status_bar_hidden(id self, SEL _cmd) {
    if (fandhe_fullscreen_desired) {
        return YES;
    }
    return fandhe_orig_status_bar_hidden
               ? ((BOOL (*)(id, SEL))fandhe_orig_status_bar_hidden)(self, _cmd)
               : NO;
}

static BOOL fandhe_prefers_home_indicator_hidden(id self, SEL _cmd) {
    if (fandhe_fullscreen_desired) {
        return YES;
    }
    return fandhe_orig_home_indicator_hidden
               ? ((BOOL (*)(id, SEL))fandhe_orig_home_indicator_hidden)(self, _cmd)
               : NO;
}

// 差し込みを 1 回だけ行う。クラスが無ければ NO（何も変更しない）。
static BOOL fandhe_install_overrides(void) {
    if (fandhe_fullscreen_installed) {
        return YES;
    }
    Class cls = objc_getClass("WinitUIViewController");
    if (cls == Nil) {
        return NO;
    }
    fandhe_orig_status_bar_hidden = class_replaceMethod(
        cls, @selector(prefersStatusBarHidden), (IMP)fandhe_prefers_status_bar_hidden, "B@:");
    fandhe_orig_home_indicator_hidden = class_replaceMethod(
        cls, @selector(prefersHomeIndicatorAutoHidden),
        (IMP)fandhe_prefers_home_indicator_hidden, "B@:");
    fandhe_fullscreen_installed = YES;
    return YES;
}

int32_t fandhe_ios_fullscreen_set(int32_t on) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        UIViewController *vc = FandheRootViewController();
        if (vc == nil || ![vc isKindOfClass:objc_getClass("WinitUIViewController")] ||
            !fandhe_install_overrides()) {
            return 3;
        }
        // 失敗経路はすべて上で返し済み。ここから先は希望値を変えても戻す必要がない。
        fandhe_fullscreen_desired = (on != 0);
        [vc setNeedsStatusBarAppearanceUpdate];
        [vc setNeedsUpdateOfHomeIndicatorAutoHidden];
    }
    return 0;
}

int32_t fandhe_ios_fullscreen_is_on(void) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        UIViewController *vc = FandheRootViewController();
        if (vc == nil || ![vc isKindOfClass:objc_getClass("WinitUIViewController")]) {
            return 3;
        }
        BOOL hidden;
        UIWindowScene *scene = vc.view.window.windowScene;
        if (scene != nil) {
            hidden = scene.statusBarManager.isStatusBarHidden;
        } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            hidden = UIApplication.sharedApplication.isStatusBarHidden;
#pragma clang diagnostic pop
        }
        BOOL wanted = vc.prefersStatusBarHidden;
        if (wanted && hidden) {
            return 1;
        }
        if (!wanted && hidden) {
            // 希望は通常なのに実状態は hidden（iPhone 横向きの OS 自動非表示、または解除の反映待ち）。
            // 通常と確認できないので未確定（3）を返し、解除成功を嘘で確定しない。
            return 3;
        }
        return 0;
    }
}
