// 画面スリープの抑止: iOS 実装（Issue #1279）。`UIApplication.idleTimerDisabled` を書く / 読む。
//
// `src/device/ios_screen_wake_lock.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（UIKit / Foundation はリンク済み）。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_ios_screen_wake_lock_set(int32_t keep) -> int32_t
//     0 = 設定した（YES の二重設定・未設定での NO も冪等で成功）。
//     2 = main スレッド以外から呼ばれた（UIKit に触れず返す。呼び出し元が `Internal` へ写す）。
//   fandhe_ios_screen_wake_lock_is_held(void) -> int32_t
//     1 = `isIdleTimerDisabled` が YES、0 = NO、2 = main スレッド以外（呼び出し元は false 扱い）。
//
// スレッド: egui の update = main から同期で呼ばれる。main 以外は `dispatch_sync` で main を
// 待たず（デッドロック回避、`device-bridge-contracts.md`「11」）2 を返す。UIKit 呼び出しは
// `@autoreleasepool` の内側で行う。
//
// 復帰: 初回の set で `UIApplicationDidBecomeActiveNotification` の observer を 1 回だけ登録し、
// 要求が YES のときに `idleTimerDisabled` を設定し直す（OS が復帰時に戻していても表示と実際を
// 揃える）。登録トークンは static の strong 参照で握り続ける（ARC で解放させない）。
// 背景にいる間は OS がアイドルタイマーを動かさず、抑止は効かない（守らない範囲）。
//
// ログは出さない（security.md A09）。プロセス終了時は OS が戻すため後片付けは不要。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

// 要求の向き。main スレッドでのみ読み書きする。
static BOOL fandhe_wake_lock_desired = NO;
static id fandhe_wake_lock_observer = nil;

int32_t fandhe_ios_screen_wake_lock_set(int32_t keep) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        fandhe_wake_lock_desired = (keep != 0);
        if (fandhe_wake_lock_observer == nil) {
            fandhe_wake_lock_observer = [[NSNotificationCenter defaultCenter]
                addObserverForName:UIApplicationDidBecomeActiveNotification
                            object:nil
                             queue:[NSOperationQueue mainQueue]
                        usingBlock:^(NSNotification *_Nonnull note __unused) {
                            if (fandhe_wake_lock_desired) {
                                UIApplication.sharedApplication.idleTimerDisabled = YES;
                            }
                        }];
        }
        UIApplication.sharedApplication.idleTimerDisabled = fandhe_wake_lock_desired;
    }
    return 0;
}

int32_t fandhe_ios_screen_wake_lock_is_held(void) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        return UIApplication.sharedApplication.isIdleTimerDisabled ? 1 : 0;
    }
}
