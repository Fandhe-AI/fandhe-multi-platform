// iOS 側 ObjC 実装（ios/*.m）が共通で使う小さなヘルパー群の置き場。
//
// `FandheTopmostViewController` は camera（Issue #751）・file_picker（Issue #873）の
// 2 ファイルに複製されていたが、共有シート（Issue #899）で 3 箇所目の利用が出たため
// ここへ抽出した（`fandhe_ios_file_picker.m` 冒頭コメントの「3 箇所目で
// fandhe_ios_common.h へ抽出する」という申し送り）。差分なしの複製をやめ、
// 挙動を 1 箇所に一本化する。
//
// `static inline` で定義するため、複数の `.m` から `#import` しても ODR 違反にならない
// （各翻訳単位に internal linkage でコピーされる、C/ObjC の慣例）。

#import <UIKit/UIKit.h>

// key window の rootViewController を返す（presented 先は辿らない）。
// 全画面表示（`fandhe_ios_fullscreen_immersive.m`）が winit の VC を直接引くために使う。
//
// 最前面（一番深く presented された）View Controller を返す。
//
// `FandheIosApp`（SwiftUI 版）は Scene 対応だが、実配布されるアプリ本体は winit
// （UIApplicationMain ベースの旧来ライフサイクル）製で、Info.plist に
// `UIApplicationSceneManifest` を持たない（`platforms/apple/FandheIosApp/Resources/
// Info.plist` 参照）。この構成では `connectedScenes` が空・もしくは目的の window を
// 含まない場合があるため、`UIApplication.shared.windows`（deprecated だが Scene 非対応
// アプリでも確実に動く）へフォールバックする（`fandhe_ios_camera.m` の元コメント参照:
// このフォールバックが無いとカメラ搭載端末でも rootViewController を取得できず UI を
// 出せない）。
static inline UIViewController *_Nullable FandheRootViewController(void) {
    UIViewController *root = nil;
    for (UIWindowScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (scene.activationState == UISceneActivationStateForegroundActive) {
            for (UIWindow *window in scene.windows) {
                if (window.isKeyWindow) {
                    root = window.rootViewController;
                    break;
                }
            }
        }
        if (root) {
            break;
        }
    }
    if (root == nil) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        root = UIApplication.sharedApplication.keyWindow.rootViewController;
        if (root == nil) {
            for (UIWindow *window in UIApplication.sharedApplication.windows) {
                if (window.isKeyWindow) {
                    root = window.rootViewController;
                    break;
                }
            }
        }
#pragma clang diagnostic pop
    }
    return root;
}

static inline UIViewController *_Nullable FandheTopmostViewController(void) {
    UIViewController *top = FandheRootViewController();
    while (top.presentedViewController) {
        top = top.presentedViewController;
    }
    return top;
}
