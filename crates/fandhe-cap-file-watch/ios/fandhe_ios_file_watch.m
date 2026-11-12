// Phase 11 の拡充ループ（親 #1143）型サンプル「フォルダの変更監視」: iOS 実装（Issue #1412）。
// open(O_EVTONLY) + DISPATCH_SOURCE_TYPE_VNODE でフォルダ 1 つの直下の変化を見張り、変化の合図だけを
// Rust へ返す。どのファイルが変わったかは読まない（Rust 側が再走査して差分を取る）。パス・ファイル名・
// errno はログに出さない。権限の宣言は増やさない（アプリのサンドボックス内の作業フォルダ）。
//
// `crates/fandhe-egui-proxy/src/device/ios_file_watch.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_ios_file_watch_start(id, path) -> int32_t
//     path（UTF-8 のフォルダパス）の監視を開始する。0 = 成功、非 0 = 失敗（何も残さない）。
//     変化のたびに専用の serial queue 上で fandhe_rust_file_watch_fire(id, kind) を呼ぶ。
//     kind 1（失敗）= 監視ルート自体の削除・移動・revoke、kind 0（変化）= それ以外。
//     OS 資源は id で所有し、生ポインタを Rust へ渡さない。
//   fandhe_ios_file_watch_stop(id)
//     停止して解放する。戻った時点で、その id のコールバックは走っておらず今後も走らない
//     （dispatch_source_cancel → cancel handler の完了を待つ。cancel handler が fd を close する）。
//     main queue へは dispatch_sync しない。未知の id は何もしない。コールバック queue 上からは
//     呼ばない（待ちで自分自身を待つため）。
//
// 守らない範囲: ディレクトリの vnode に出ない「rename なしの既存ファイルの中身の書き換え」、
// バックグラウンドでの一時停止中の遅れ。
//
// FFI 規約: 各関数を `@autoreleasepool` で囲む。辞書の操作は @synchronized で守り、ロックの内側では待たない。

#import <Foundation/Foundation.h>
#include <fcntl.h>
#include <stdint.h>
#include <unistd.h>

// Rust 側の受け口（device/file_watch.rs）。kind 0 = 変化の合図、それ以外 = 失敗。
extern void fandhe_rust_file_watch_fire(uint64_t id, int32_t kind);

@interface FandheIosFsWatch : NSObject
@property(nonatomic, strong) dispatch_source_t source;
@property(nonatomic, strong) dispatch_semaphore_t cancelled;
@end
@implementation FandheIosFsWatch
@end

static NSMutableDictionary<NSNumber *, FandheIosFsWatch *> *g_watches;

static NSMutableDictionary<NSNumber *, FandheIosFsWatch *> *watches(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        g_watches = [NSMutableDictionary dictionary];
    });
    return g_watches;
}

int32_t fandhe_ios_file_watch_start(uint64_t id, const char *path) {
    if (path == NULL) {
        return 1;
    }
    @autoreleasepool {
        NSMutableDictionary *map = watches();
        @synchronized(map) {
            if (map[@(id)] != nil) {
                return 2;
            }
        }
        int fd = open(path, O_EVTONLY);
        if (fd < 0) {
            return 3;
        }
        dispatch_queue_t queue = dispatch_queue_create("com.fandhe.file-watch", DISPATCH_QUEUE_SERIAL);
        dispatch_source_t src = dispatch_source_create(
            DISPATCH_SOURCE_TYPE_VNODE, (uintptr_t)fd,
            DISPATCH_VNODE_WRITE | DISPATCH_VNODE_EXTEND | DISPATCH_VNODE_ATTRIB |
                DISPATCH_VNODE_LINK | DISPATCH_VNODE_DELETE | DISPATCH_VNODE_RENAME |
                DISPATCH_VNODE_REVOKE,
            queue);
        if (src == NULL) {
            close(fd);
            return 4;
        }
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        // ハンドラは source を強参照しない（循環参照を避ける）。
        __weak dispatch_source_t weakSrc = src;
        dispatch_source_set_event_handler(src, ^{
            @autoreleasepool {
                dispatch_source_t s = weakSrc;
                if (s == nil) {
                    return;
                }
                unsigned long flags = dispatch_source_get_data(s);
                int32_t kind = (flags & (DISPATCH_VNODE_DELETE | DISPATCH_VNODE_RENAME |
                                         DISPATCH_VNODE_REVOKE))
                                   ? 1
                                   : 0;
                fandhe_rust_file_watch_fire(id, kind);
            }
        });
        dispatch_source_set_cancel_handler(src, ^{
            close(fd);
            dispatch_semaphore_signal(done);
        });
        FandheIosFsWatch *w = [FandheIosFsWatch new];
        w.source = src;
        w.cancelled = done;
        @synchronized(map) {
            if (map[@(id)] != nil) {
                // 競合で同じ id が先に入った。cancel handler を走らせて fd を閉じる。
                dispatch_resume(src);
                dispatch_source_cancel(src);
                return 2;
            }
            map[@(id)] = w;
        }
        dispatch_resume(src);
        return 0;
    }
}

void fandhe_ios_file_watch_stop(uint64_t id) {
    @autoreleasepool {
        NSMutableDictionary *map = watches();
        FandheIosFsWatch *w = nil;
        @synchronized(map) {
            w = map[@(id)];
            [map removeObjectForKey:@(id)];
        }
        if (w == nil) {
            return;
        }
        dispatch_source_cancel(w.source);
        // 実行中ハンドラの終了と fd の close を待つ（cancel handler は同じ queue で最後に走る）。
        dispatch_semaphore_wait(w.cancelled, DISPATCH_TIME_FOREVER);
    }
}
