// Phase 11 の拡充ループ（親 #179）型サンプル「メモリ逼迫・負荷」: iOS 実装（Issue #912）。
// メモリ警告（UIApplicationDidReceiveMemoryWarningNotification、main キューの observer）と、
// libdispatch の DISPATCH_SOURCE_TYPE_MEMORYPRESSURE（normal / warn / critical の変化）を受け、
// 値をそのまま Rust へ返す。起動時の段階を合図として捏造しない。今の段階を読む公開 API は無い。
// 権限の宣言は増やさない。
//
// `crates/fandhe-egui-proxy/src/device/ios_memory_pressure.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_ios_memory_pressure_start(id) -> int32_t
//     メモリ警告の observer と dispatch source を作って resume する。0 = 成功、非 0 = 失敗
//     （何も残さない）。届くたびに fandhe_rust_memory_pressure_fire(sub_id, kind, flags) を呼ぶ。
//     kind 0 = dispatch source（flags = data: 0x1 / 0x2 / 0x4）、kind 1 = メモリ警告（flags = 0）。
//     ハンドラは呼ぶだけですぐ戻る。OS 資源は id で所有し、生ポインタを Rust へ渡さない。
//     同じ id の二重登録は失敗。main へ dispatch_sync しない。
//   fandhe_ios_memory_pressure_stop(id)
//     辞書から外してから observer を外し、source を cancel する。未知の id は何もしない。
//     実行中のハンドラは Rust 側の登録表で捨てられる。
//
// 守らない範囲: 背景で停止中の合図、通知の発行スレッド（公式に記載なし）。
//
// FFI 規約: 各関数を `@autoreleasepool` で囲む。辞書の操作は @synchronized で守り、ロックの内側では待たない。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#include <dispatch/dispatch.h>
#include <stdint.h>

// Rust 側の受け口（device/memory_pressure.rs）。
extern void fandhe_rust_memory_pressure_fire(uint64_t sub_id, int32_t kind, uint64_t flags);

// 値は @[observer トークン, dispatch source]。
static NSMutableDictionary<NSNumber *, NSArray *> *g_entries;

static NSMutableDictionary<NSNumber *, NSArray *> *entries(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        g_entries = [NSMutableDictionary dictionary];
    });
    return g_entries;
}

// 未 resume のまま解放すると落ちるため、resume してから cancel する。
static void discard_source(dispatch_source_t src) {
    dispatch_source_set_event_handler(src, nil);
    dispatch_resume(src);
    dispatch_source_cancel(src);
}

int32_t fandhe_ios_memory_pressure_start(uint64_t sub_id) {
    @autoreleasepool {
        NSMutableDictionary *map = entries();
        @synchronized(map) {
            if (map[@(sub_id)] != nil) {
                return 2;
            }
        }
        dispatch_queue_t queue = dispatch_queue_create("fandhe.memory_pressure", DISPATCH_QUEUE_SERIAL);
        if (queue == nil) {
            return 3;
        }
        dispatch_source_t src = dispatch_source_create(
            DISPATCH_SOURCE_TYPE_MEMORYPRESSURE, 0,
            DISPATCH_MEMORYPRESSURE_NORMAL | DISPATCH_MEMORYPRESSURE_WARN | DISPATCH_MEMORYPRESSURE_CRITICAL,
            queue);
        if (src == nil) {
            return 4;
        }
        // 強参照で捕捉すると source ⇄ ハンドラの循環になり、stop 後も解放されない。弱参照で捕捉する。
        __weak dispatch_source_t weakSrc = src;
        dispatch_source_set_event_handler(src, ^{
            dispatch_source_t s = weakSrc;
            if (s == nil) {
                return;
            }
            fandhe_rust_memory_pressure_fire(sub_id, 0, (uint64_t)dispatch_source_get_data(s));
        });
        id token = [[NSNotificationCenter defaultCenter]
            addObserverForName:UIApplicationDidReceiveMemoryWarningNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *__unused note) {
                        fandhe_rust_memory_pressure_fire(sub_id, 1, 0);
                    }];
        if (token == nil) {
            discard_source(src);
            return 5;
        }
        // 辞書へ保存してから resume する（保存前に届いた合図で停止と競合しない）。
        @synchronized(map) {
            if (map[@(sub_id)] != nil) {
                [[NSNotificationCenter defaultCenter] removeObserver:token];
                discard_source(src);
                return 2;
            }
            map[@(sub_id)] = @[ token, src ];
        }
        dispatch_resume(src);
        return 0;
    }
}

void fandhe_ios_memory_pressure_stop(uint64_t sub_id) {
    @autoreleasepool {
        NSMutableDictionary *map = entries();
        NSArray *entry = nil;
        @synchronized(map) {
            entry = map[@(sub_id)];
            [map removeObjectForKey:@(sub_id)];
        }
        if (entry == nil) {
            return;
        }
        [[NSNotificationCenter defaultCenter] removeObserver:entry[0]];
        dispatch_source_t src = entry[1];
        // ハンドラを外してから cancel し、保持関係を確実に断つ。
        dispatch_source_set_event_handler(src, nil);
        dispatch_source_cancel(src);
    }
}
