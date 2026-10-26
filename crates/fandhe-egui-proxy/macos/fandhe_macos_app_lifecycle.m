// Phase 11 の拡充ループ（親 #178）型サンプル「アプリの前面・背面」: macOS 実装（Issue #893）。
// NSApplication の通知で、アプリが操作の対象になった・外れたこと（didBecomeActive / didResignActive）と、
// 見える・見えなくなったこと（didChangeOcclusionState。受けたらその場で occlusionState を問い直す）を受け、
// 出来事のコードだけを Rust へ返す。didHide / didUnhide は occlusion に含まれるため別に取らない。
// 凍結（App Nap）の通知は無い。権限の宣言は増やさない。
//
// `crates/fandhe-egui-proxy/src/device/macos_app_lifecycle.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_app_lifecycle_start(id) -> int32_t
//     3 通知の observer を登録する。0 = 成功、非 0 = 失敗（登録済みの分を全て外し、何も残さない）。
//     届くたびに fandhe_rust_app_lifecycle_fire(sub_id, code) を呼ぶ。code: 0 = 操作の対象になった、
//     1 = 外れた、2 = 見えなくなった、3 = 見えるようになった。ブロックは呼ぶだけですぐ戻る。
//     OS 資源は id で所有し、生ポインタを Rust へ渡さない。同じ id の二重登録は失敗。
//     main へ dispatch_sync しない。
//   fandhe_macos_app_lifecycle_stop(id)
//     observer を全て外して解放する。未知の id は何もしない。stop の後に新しい通知は配送されない
//     （既に実行中のブロックは短く、Rust 側の登録表で捨てられる）。
//   fandhe_macos_app_lifecycle_read() -> int32_t
//     今の状態の符号化。bit 0-1 = 操作の対象か（0 = いいえ / 1 = はい）、bit 2-3 = 見えているか
//     （0 = いいえ / 1 = はい）。main thread でなければ -1、NSApp が nil なら -2。
//
// FFI 規約: 各関数を `@autoreleasepool` で囲む。辞書の操作は @synchronized で守り、ロックの内側では待たない。

#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
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

int32_t fandhe_macos_app_lifecycle_start(uint64_t sub_id) {
    @autoreleasepool {
        NSMutableDictionary *map = observers();
        @synchronized(map) {
            if (map[@(sub_id)] != nil) {
                return 2;
            }
        }
        if (NSApp == nil) {
            return 3;
        }
        NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
        NSMutableArray *tokens = [NSMutableArray arrayWithCapacity:3];

        NSArray<NSNotificationName> *direct = @[
            NSApplicationDidBecomeActiveNotification,
            NSApplicationDidResignActiveNotification,
        ];
        for (NSUInteger i = 0; i < direct.count; i++) {
            int32_t code = (int32_t)i;
            id token = [center addObserverForName:direct[i]
                                           object:NSApp
                                            queue:nil
                                       usingBlock:^(NSNotification *note) {
                                           (void)note;
                                           fandhe_rust_app_lifecycle_fire(sub_id, code);
                                       }];
            if (token == nil) {
                for (id t in tokens) {
                    [center removeObserver:t];
                }
                return 4;
            }
            [tokens addObject:token];
        }

        // 見える・見えないは通知のたびに occlusionState を問い直して決める（通知名だけでは分からない）。
        id occlusion = [center addObserverForName:NSApplicationDidChangeOcclusionStateNotification
                                           object:NSApp
                                            queue:nil
                                       usingBlock:^(NSNotification *note) {
                                           (void)note;
                                           BOOL visible = ([NSApp occlusionState] & NSApplicationOcclusionStateVisible) != 0;
                                           fandhe_rust_app_lifecycle_fire(sub_id, visible ? 3 : 2);
                                       }];
        if (occlusion == nil) {
            for (id t in tokens) {
                [center removeObserver:t];
            }
            return 5;
        }
        [tokens addObject:occlusion];

        @synchronized(map) {
            map[@(sub_id)] = tokens;
        }
        return 0;
    }
}

void fandhe_macos_app_lifecycle_stop(uint64_t sub_id) {
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

int32_t fandhe_macos_app_lifecycle_read(void) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return -1;
        }
        if (NSApp == nil) {
            return -2;
        }
        int32_t active = [NSApp isActive] ? 1 : 0;
        int32_t visible = ([NSApp occlusionState] & NSApplicationOcclusionStateVisible) != 0 ? 1 : 0;
        return active | (visible << 2);
    }
}
