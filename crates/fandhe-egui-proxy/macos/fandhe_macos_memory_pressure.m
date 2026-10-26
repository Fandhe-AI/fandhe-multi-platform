// Phase 11 の拡充ループ（親 #179）型サンプル「メモリ逼迫・負荷」: macOS 実装（Issue #911）。
// libdispatch の DISPATCH_SOURCE_TYPE_MEMORYPRESSURE（normal / warn / critical の変化）を受け、
// dispatch source の data（NORMAL=0x1 / WARN=0x2 / CRITICAL=0x4）をそのまま Rust へ返す。
// 起動時の段階を合図として捏造しない（変化したときだけ届く）。今の段階を読む公開 API は無く、
// 非公開の sysctl は使わない。権限の宣言は増やさない。
//
// `crates/fandhe-egui-proxy/src/device/macos_memory_pressure.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_memory_pressure_start(id) -> int32_t
//     専用 serial queue で dispatch source を作って resume する。0 = 成功、非 0 = 失敗
//     （何も残さない）。届くたびに fandhe_rust_memory_pressure_fire(sub_id, 0, data) を呼ぶ。
//     ハンドラは呼ぶだけですぐ戻る。OS 資源は id で所有し、生ポインタを Rust へ渡さない。
//     同じ id の二重登録は失敗。main へ dispatch_sync しない。
//   fandhe_macos_memory_pressure_stop(id)
//     辞書から外してから dispatch_source_cancel する。未知の id は何もしない。cancel は非同期で、
//     実行中のハンドラは Rust 側の登録表で捨てられる。
//
// FFI 規約: 各関数を `@autoreleasepool` で囲む。辞書の操作は @synchronized で守り、ロックの内側では待たない。

#import <Foundation/Foundation.h>
#include <dispatch/dispatch.h>
#include <stdint.h>

// Rust 側の受け口（device/memory_pressure.rs）。
extern void fandhe_rust_memory_pressure_fire(uint64_t sub_id, int32_t kind, uint64_t flags);

static NSMutableDictionary<NSNumber *, dispatch_source_t> *g_sources;

static NSMutableDictionary<NSNumber *, dispatch_source_t> *sources(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        g_sources = [NSMutableDictionary dictionary];
    });
    return g_sources;
}

int32_t fandhe_macos_memory_pressure_start(uint64_t sub_id) {
    @autoreleasepool {
        NSMutableDictionary *map = sources();
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
        dispatch_source_set_event_handler(src, ^{
            fandhe_rust_memory_pressure_fire(sub_id, 0, (uint64_t)dispatch_source_get_data(src));
        });
        // 辞書へ保存してから resume する（保存前に届いた合図で停止と競合しない）。
        @synchronized(map) {
            if (map[@(sub_id)] != nil) {
                // 未 resume のまま解放すると落ちるため、resume してから cancel する。
                dispatch_resume(src);
                dispatch_source_cancel(src);
                return 2;
            }
            map[@(sub_id)] = src;
        }
        dispatch_resume(src);
        return 0;
    }
}

void fandhe_macos_memory_pressure_stop(uint64_t sub_id) {
    @autoreleasepool {
        NSMutableDictionary *map = sources();
        dispatch_source_t src = nil;
        @synchronized(map) {
            src = map[@(sub_id)];
            [map removeObjectForKey:@(sub_id)];
        }
        if (src == nil) {
            return;
        }
        dispatch_source_cancel(src);
    }
}
