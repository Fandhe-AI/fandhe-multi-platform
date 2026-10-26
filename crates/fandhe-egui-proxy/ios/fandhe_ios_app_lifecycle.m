// Phase 11 の拡充ループ（親 #178）型サンプル「アプリの前面・背面」: iOS 実装（Issue #894）。
// UIScene の 4 通知（didActivate = 0 / willDeactivate = 1 / didEnterBackground = 2 / willEnterForeground = 3）を
// 受け、出来事のコードだけを Rust へ返す。本アプリは scene 対応のため UIApplication の通知は使わない。
// 見るのは application ロールの scene だけで、外部画面の scene は捨てる。権限の宣言は増やさない。
//
// `crates/fandhe-egui-proxy/src/device/ios_app_lifecycle.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_ios_app_lifecycle_start(id) -> int32_t
//     4 通知の observer を登録する。0 = 成功、非 0 = 失敗（登録済みの分を全て外し、何も残さない）。
//     届くたびに fandhe_rust_app_lifecycle_fire(sub_id, code) を呼ぶ。ブロックは呼ぶだけですぐ戻る。
//     object:nil で登録するため、開始時点の scene の有無に左右されず、scene が付け直されても追従する。
//     OS 資源は id で所有し、生ポインタを Rust へ渡さない。同じ id の二重登録は失敗。
//     main へ dispatch_sync しない。
//   fandhe_ios_app_lifecycle_stop(id)
//     observer を全て外して解放する。未知の id は何もしない。stop の後に新しい通知は配送されない
//     （既に実行中のブロックは短く、Rust 側の登録表で捨てられる）。
//   fandhe_ios_app_lifecycle_read() -> int32_t
//     今の状態の符号化（current_from_apple_bits と同じ）。bit 0-1 = 操作の対象か（0 = いいえ / 1 = はい /
//     2 = 不明）、bit 2-3 = 見えているか（同）。ForegroundActive = 5、ForegroundInactive = 4、Background = 0、
//     scene 未接続・Unattached = 10（不明）。main thread でなければ -1、規定外の activationState は -2。
//
// 守らない範囲: suspended へ移る通知は無い。willEnterForeground の直後に読んだ状態は、まだ Background の
// ことがある（通知と OS の実状態の時間差）。
//
// FFI 規約: 各関数を `@autoreleasepool` で囲む。辞書の操作は @synchronized で守り、ロックの内側では待たない。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#include <stdint.h>

// Rust 側の受け口（device/app_lifecycle.rs）。
extern void fandhe_rust_app_lifecycle_fire(uint64_t sub_id, int32_t code);

static NSMutableDictionary<NSNumber *, NSArray *> *g_observers;

static NSMutableDictionary<NSNumber *, NSArray *> *observers(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        g_observers = [NSMutableDictionary dictionary];
    });
    return g_observers;
}

// application ロールの UIWindowScene か（外部画面の scene を除く）。
static BOOL is_application_scene(id obj) {
    return [obj isKindOfClass:[UIWindowScene class]]
        && [((UIWindowScene *)obj).session.role isEqualToString:UIWindowSceneSessionRoleApplication];
}

int32_t fandhe_ios_app_lifecycle_start(uint64_t sub_id) {
    @autoreleasepool {
        NSMutableDictionary *map = observers();
        @synchronized(map) {
            if (map[@(sub_id)] != nil) {
                return 2;
            }
        }
        NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
        NSArray<NSNotificationName> *names = @[
            UISceneDidActivateNotification,
            UISceneWillDeactivateNotification,
            UISceneDidEnterBackgroundNotification,
            UISceneWillEnterForegroundNotification,
        ];
        NSMutableArray *tokens = [NSMutableArray arrayWithCapacity:names.count];
        for (NSUInteger i = 0; i < names.count; i++) {
            int32_t code = (int32_t)i;
            id token = [center addObserverForName:names[i]
                                           object:nil
                                            queue:nil
                                       usingBlock:^(NSNotification *note) {
                                           if (is_application_scene(note.object)) {
                                               fandhe_rust_app_lifecycle_fire(sub_id, code);
                                           }
                                       }];
            if (token == nil) {
                for (id t in tokens) {
                    [center removeObserver:t];
                }
                return 4;
            }
            [tokens addObject:token];
        }
        @synchronized(map) {
            map[@(sub_id)] = tokens;
        }
        return 0;
    }
}

void fandhe_ios_app_lifecycle_stop(uint64_t sub_id) {
    @autoreleasepool {
        NSMutableDictionary *map = observers();
        NSArray *tokens = nil;
        @synchronized(map) {
            tokens = map[@(sub_id)];
            [map removeObjectForKey:@(sub_id)];
        }
        if (tokens == nil) {
            return;
        }
        NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
        for (id t in tokens) {
            [center removeObserver:t];
        }
    }
}

int32_t fandhe_ios_app_lifecycle_read(void) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return -1;
        }
        UIWindowScene *app_scene = nil;
        for (UIScene *scene in [UIApplication sharedApplication].connectedScenes) {
            if (is_application_scene(scene)) {
                app_scene = (UIWindowScene *)scene;
                break;
            }
        }
        if (app_scene == nil) {
            return 2 | (2 << 2);
        }
        switch (app_scene.activationState) {
            case UISceneActivationStateForegroundActive:
                return 1 | (1 << 2);
            case UISceneActivationStateForegroundInactive:
                return 0 | (1 << 2);
            case UISceneActivationStateBackground:
                return 0;
            case UISceneActivationStateUnattached:
                return 2 | (2 << 2);
            default:
                return -2;
        }
    }
}
