// Phase 11 の拡充ループ（親 #167）型サンプル「スリープ・ロックの通知」: macOS 実装（Issue #810）。
// NSWorkspace の通知センターで、システムのスリープ・復帰と画面のスリープ・復帰の 4 通知を受け、
// 出来事のコードだけを Rust へ返す。画面ロックを知らせる公開 API は無いため、分散通知
// （com.apple.screenIsLocked 等）や CGSessionCopyCurrentDictionary は使わない（ロックは取れない扱い）。
// 権限の宣言は増やさない。
//
// `crates/fandhe-egui-proxy/src/device/macos_sleep_lock_events.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_sleep_lock_events_start(id) -> int32_t
//     4 通知の observer を登録する。0 = 成功、非 0 = 失敗（登録済みの分を全て外し、何も残さない）。
//     届くたびに fandhe_rust_sleep_lock_events_fire(sub_id, code) を呼ぶ。code: 0 = WillSleep、
//     1 = DidWake、2 = ScreensDidSleep、3 = ScreensDidWake。ブロックは呼ぶだけですぐ戻る
//     （willSleep の処理中はスリープを最大 30 秒遅らせうるため、公式）。
//     OS 資源は id で所有し、生ポインタを Rust へ渡さない。同じ id の二重登録は失敗。
//     呼び出しスレッドは任意（NSNotificationCenter はスレッドセーフ）。main へ dispatch_sync しない。
//   fandhe_macos_sleep_lock_events_stop(id)
//     observer を全て外して解放する。未知の id は何もしない。stop の後に新しい通知は配送されない
//     （既に実行中のブロックは短く、Rust 側の登録表で捨てられる）。
//
// FFI 規約: 各関数を `@autoreleasepool` で囲む。辞書の操作は @synchronized で守り、ロックの内側では待たない。

#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#include <stdint.h>

// Rust 側の受け口（device/sleep_lock_events.rs）。
extern void fandhe_rust_sleep_lock_events_fire(uint64_t sub_id, int32_t code);

static NSMutableDictionary<NSNumber *, NSArray *> *g_observers;

static NSMutableDictionary<NSNumber *, NSArray *> *observers(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        g_observers = [NSMutableDictionary dictionary];
    });
    return g_observers;
}

int32_t fandhe_macos_sleep_lock_events_start(uint64_t sub_id) {
    @autoreleasepool {
        NSMutableDictionary *map = observers();
        @synchronized(map) {
            if (map[@(sub_id)] != nil) {
                return 2;
            }
        }
        NSNotificationCenter *center = [NSWorkspace sharedWorkspace].notificationCenter;
        if (center == nil) {
            return 3;
        }
        NSArray<NSNotificationName> *names = @[
            NSWorkspaceWillSleepNotification,
            NSWorkspaceDidWakeNotification,
            NSWorkspaceScreensDidSleepNotification,
            NSWorkspaceScreensDidWakeNotification,
        ];
        NSMutableArray *tokens = [NSMutableArray arrayWithCapacity:names.count];
        for (NSUInteger i = 0; i < names.count; i++) {
            int32_t code = (int32_t)i;
            id token = [center addObserverForName:names[i]
                                           object:nil
                                            queue:nil
                                       usingBlock:^(NSNotification *note) {
                                           (void)note;
                                           fandhe_rust_sleep_lock_events_fire(sub_id, code);
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

void fandhe_macos_sleep_lock_events_stop(uint64_t sub_id) {
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
        NSNotificationCenter *center = [NSWorkspace sharedWorkspace].notificationCenter;
        for (id t in tokens) {
            [center removeObserver:t];
        }
    }
}
