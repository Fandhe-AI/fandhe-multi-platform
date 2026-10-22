// 画面キャプチャの検知: iOS 実装（Issue #583）。録画・ミラーリングの状態を読み、スクリーンショット通知を数える。
//
// `src/device/ios_capture_protection.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐で
// `cc` コンパイルする（UIKit / Foundation はリンク済み）。iOS に防止の公式 API は無く、防止は実装しない。
//
// 契約:
//   fandhe_ios_capture_protection_read_recording(void) -> int32_t
//     0 = 録画・ミラーリングされていない、1 = されている、
//     2 = main スレッド以外から呼ばれた（UIKit に触れず返す）、
//     3 = 読めない（背景にいる・foreground の UIWindowScene が無い・Unspecified・iOS 17 未満）。
//     呼び出し元は 2・3・未知値を「不明」として扱い、false に畳まない。
//   fandhe_ios_capture_protection_start_screenshot(void) -> int32_t
//     0 = 登録済み（2 回目以降は何もしない冪等）、2 = main スレッド以外。
//
// スレッド: egui の update = main から同期で呼ばれる。`dispatch_sync` は使わない（契約表 11、デッドロック回避）。
// UIKit 呼び出しは `@autoreleasepool` の内側で行う。
//
// observer: 登録トークンは static の strong 参照で握り続ける（ARC で解放させない）。通知は main キューで
// 受け、Rust 側の `fandhe_rust_capture_protection_screenshot_taken` を呼ぶだけ（回数の確定と再描画は Rust 側）。
//
// ログは出さない（security.md A09）。observer はプロセスと寿命が同じで、終了時は OS が片付けるため解除しない。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

extern void fandhe_rust_capture_protection_screenshot_taken(void);

static id fandhe_capture_screenshot_observer = nil;

int32_t fandhe_ios_capture_protection_read_recording(void) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        // 背景中は最後の値を返し続けない（契約表 1）。
        if (UIApplication.sharedApplication.applicationState == UIApplicationStateBackground) {
            return 3;
        }
        if (@available(iOS 17.0, *)) {
            UIWindowScene *active = nil;
            UIWindowScene *inactive = nil;
            for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
                if (![scene isKindOfClass:[UIWindowScene class]]) {
                    continue;
                }
                if (scene.activationState == UISceneActivationStateForegroundActive) {
                    active = (UIWindowScene *)scene;
                    break;
                }
                if (scene.activationState == UISceneActivationStateForegroundInactive && inactive == nil) {
                    inactive = (UIWindowScene *)scene;
                }
            }
            UIWindowScene *target = active != nil ? active : inactive;
            if (target == nil) {
                return 3;
            }
            switch (target.traitCollection.sceneCaptureState) {
                case UISceneCaptureStateActive:
                    return 1;
                case UISceneCaptureStateInactive:
                    return 0;
                default:
                    return 3;
            }
        }
        return 3;
    }
}

int32_t fandhe_ios_capture_protection_start_screenshot(void) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        if (fandhe_capture_screenshot_observer == nil) {
            fandhe_capture_screenshot_observer = [[NSNotificationCenter defaultCenter]
                addObserverForName:UIApplicationUserDidTakeScreenshotNotification
                            object:nil
                             queue:[NSOperationQueue mainQueue]
                        usingBlock:^(NSNotification *_Nonnull note __unused) {
                            fandhe_rust_capture_protection_screenshot_taken();
                        }];
        }
    }
    return 0;
}
