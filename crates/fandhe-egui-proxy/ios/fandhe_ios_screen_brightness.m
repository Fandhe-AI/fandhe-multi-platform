// 画面輝度の取得・設定: iOS 実装（Issue #634、親 #126）。`UIScreen.brightness` を、アプリが
// 前面にいる間だけ上書きする。
//
// `src/device/ios_screen_brightness.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（UIKit / Foundation はリンク済み）。
//
// 契約（戻りコードは値渡しの int32_t）:
//   fandhe_ios_screen_brightness_set(double level, double tolerance) -> int32_t
//   fandhe_ios_screen_brightness_clear(void) -> int32_t
//   fandhe_ios_screen_brightness_read(double *out_level, int32_t *out_overriding) -> int32_t
//     0 = 成功
//     2 = main スレッド以外から呼ばれた（UIKit に触れず返す。`dispatch_sync` はしない）
//     3 = 前面の scene・screen が取れない
//     4 = 引数が範囲外・非有限（Rust 側で先に弾くので防御用）
//   `tolerance` は Rust の `NEAR_EQUAL_TOLERANCE`（仮の値）を引数で受ける。値は Rust が正。
//   Rust 側が定義する `fandhe_rust_screen_brightness_changed(void)` を、状態を更新した直後に呼ぶ
//   （egui は入力が無いと描画しないため、`crate::ios::request_repaint` を起こす）。
//
// スレッド: 状態（元の値・要求値・フラグ）と observer は main スレッドだけで扱う。入口と block は
// すべて `@autoreleasepool` で包む。画面は前面の `UIWindowScene.screen` から取る
// （`UIScreen.mainScreen` は使わない）。
//
// ライフサイクル:
//   - didEnterBackground で元の値へ戻し（gInBackground=YES）、WillEnterForeground /
//     DidBecomeActive で掛け直す。戻している間も read は overriding=1 と要求値を返す。
//   - overriding=0 にするのは、利用者による変更を検知して上書きをやめたときだけ。
//     検知は通知 block と read の両方で行う（通知の取りこぼしの保険）。
//   - clear に失敗（screen が取れない）したら gOverriding は YES のまま残す。
//
// observer: 初回の set で 1 回だけ登録し、static の strong 参照で握り続ける。プロセスと寿命が
// 同じで終了時は OS が片付けるため解除しない。
//
// ログは出さない（security.md A09）。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"

// Rust 側が定義する再描画の受け口。
extern void fandhe_rust_screen_brightness_changed(void);

static BOOL gOverriding = NO;
static BOOL gInBackground = NO;
static CGFloat gOriginal = 0.0;
static CGFloat gDesired = 0.0;
static CGFloat gTolerance = 0.02;
static __weak UIScreen *gScreen = nil;
static NSArray<id> *gObservers = nil;

// 前面の scene の screen。取れなければ nil。
static UIScreen *_Nullable FandheBrightnessScreen(void) {
    UIScreen *screen = FandheRootViewController().view.window.windowScene.screen;
    if (screen != nil) {
        gScreen = screen;
        return screen;
    }
    return nil;
}

// 背景中に戻すための screen。前面で取れなければ最後に控えたものを使う。
static UIScreen *_Nullable FandheBrightnessAnyScreen(void) {
    UIScreen *screen = FandheBrightnessScreen();
    return screen != nil ? screen : gScreen;
}

// 利用者による変更を検知したら上書きをやめる（利用者の値は残し、元の値へは戻さない）。
static void FandheBrightnessDetectUserChange(UIScreen *screen) {
    if (gOverriding && !gInBackground &&
        fabs((double)screen.brightness - (double)gDesired) > (double)gTolerance) {
        gOverriding = NO;
    }
}

static void FandheBrightnessEnterBackground(void) {
    if (!gOverriding || gInBackground) {
        return;
    }
    UIScreen *screen = FandheBrightnessAnyScreen();
    if (screen != nil) {
        screen.brightness = gOriginal;
    }
    gInBackground = YES;
    fandhe_rust_screen_brightness_changed();
}

static void FandheBrightnessEnterForeground(void) {
    if (!gOverriding || !gInBackground) {
        return;
    }
    UIScreen *screen = FandheBrightnessScreen();
    if (screen == nil) {
        return;  // 背景のまま残し、DidBecomeActive で再試行する。
    }
    gOriginal = screen.brightness;  // 背景中に利用者が変えた値を取り直す。
    screen.brightness = gDesired;
    gInBackground = NO;
    fandhe_rust_screen_brightness_changed();
}

static void FandheBrightnessRegisterObservers(void) {
    if (gObservers != nil) {
        return;
    }
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    NSOperationQueue *queue = NSOperationQueue.mainQueue;
    id o1 = [center addObserverForName:UIScreenBrightnessDidChangeNotification
                                object:nil
                                 queue:queue
                            usingBlock:^(NSNotification *_Nonnull note __unused) {
                                @autoreleasepool {
                                    UIScreen *screen = FandheBrightnessScreen();
                                    if (screen == nil) {
                                        return;
                                    }
                                    BOOL before = gOverriding;
                                    FandheBrightnessDetectUserChange(screen);
                                    if (before != gOverriding) {
                                        fandhe_rust_screen_brightness_changed();
                                    }
                                }
                            }];
    id o2 = [center addObserverForName:UIApplicationDidEnterBackgroundNotification
                                object:nil
                                 queue:queue
                            usingBlock:^(NSNotification *_Nonnull note __unused) {
                                @autoreleasepool {
                                    FandheBrightnessEnterBackground();
                                }
                            }];
    id o3 = [center addObserverForName:UIApplicationWillEnterForegroundNotification
                                object:nil
                                 queue:queue
                            usingBlock:^(NSNotification *_Nonnull note __unused) {
                                @autoreleasepool {
                                    FandheBrightnessEnterForeground();
                                }
                            }];
    id o4 = [center addObserverForName:UIApplicationDidBecomeActiveNotification
                                object:nil
                                 queue:queue
                            usingBlock:^(NSNotification *_Nonnull note __unused) {
                                @autoreleasepool {
                                    FandheBrightnessEnterForeground();
                                }
                            }];
    gObservers = @[ o1, o2, o3, o4 ];
}

int32_t fandhe_ios_screen_brightness_set(double level, double tolerance) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        if (!isfinite(level) || level < 0.0 || level > 1.0 || !isfinite(tolerance) ||
            tolerance < 0.0) {
            return 4;
        }
        UIScreen *screen = FandheBrightnessScreen();
        if (screen == nil) {
            return 3;
        }
        FandheBrightnessRegisterObservers();
        if (!gOverriding) {
            gOriginal = screen.brightness;
            gInBackground = NO;
        }
        gDesired = (CGFloat)level;
        gTolerance = (CGFloat)tolerance;
        gOverriding = YES;
        BOOL background =
            gInBackground ||
            UIApplication.sharedApplication.applicationState == UIApplicationStateBackground;
        if (background) {
            gInBackground = YES;  // 書き込まず、前面に戻ったときに掛ける。
        } else {
            screen.brightness = gDesired;
        }
    }
    return 0;
}

int32_t fandhe_ios_screen_brightness_clear(void) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        if (!gOverriding) {
            return 0;
        }
        if (gInBackground) {
            gOverriding = NO;  // すでに利用者の値へ戻してある。
            gInBackground = NO;
            return 0;
        }
        UIScreen *screen = FandheBrightnessScreen();
        if (screen == nil) {
            return 3;  // gOverriding は YES のまま残す。
        }
        screen.brightness = gOriginal;
        gOverriding = NO;
    }
    return 0;
}

int32_t fandhe_ios_screen_brightness_read(double *out_level, int32_t *out_overriding) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    if (out_level == NULL || out_overriding == NULL) {
        return 4;
    }
    @autoreleasepool {
        if (gOverriding && gInBackground) {
            *out_level = (double)gDesired;
            *out_overriding = 1;
            return 0;
        }
        UIScreen *screen = FandheBrightnessScreen();
        if (screen == nil) {
            return 3;
        }
        FandheBrightnessDetectUserChange(screen);
        *out_level = (double)screen.brightness;
        *out_overriding = gOverriding ? 1 : 0;
    }
    return 0;
}
