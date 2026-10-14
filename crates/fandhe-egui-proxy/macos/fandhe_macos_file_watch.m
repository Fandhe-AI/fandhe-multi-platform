// Phase 11 の拡充ループ（親 #1143）型サンプル「フォルダの変更監視」: macOS 実装（Issue #1410）。
// CoreServices の FSEventStream でフォルダ 1 つの変化を見張り、変化の合図だけを Rust へ返す。
// どのファイルが変わったかは読まない（Rust 側が再走査して差分を取る）。あふれ・取りこぼし
// （MustScanSubDirs / *Dropped 等）も同じ合図に畳む。パス・ファイル名はログに出さない。権限の宣言は
// 増やさない（アプリ専用の作業フォルダ。App Sandbox は使っていない）。
//
// `crates/fandhe-egui-proxy/src/device/macos_file_watch.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_file_watch_start(id, path) -> int32_t
//     path（UTF-8 の実パス）の監視を開始する。0 = 成功、非 0 = 失敗（何も残さない）。
//     変化のたびに専用の serial queue 上で fandhe_rust_file_watch_fire(id, 0) を呼ぶ。
//     OS 資源は id で所有し、生ポインタを Rust へ渡さない。
//   fandhe_macos_file_watch_stop(id)
//     停止して解放する。戻った時点で、その id のコールバックは走っておらず今後も走らない
//     （停止 → 無効化 → 専用 queue の空 dispatch_sync で実行中のコールバックの終了を待つ → 解放）。
//     main queue へは dispatch_sync しない。未知の id は何もしない。呼び出しスレッドは任意だが、
//     コールバック queue 上からは呼ばない（待ちで自分自身を待つため）。
//
// FFI 規約: 各関数を `@autoreleasepool` で囲む。辞書の操作は @synchronized で守り、ロックの内側では待たない。

#import <CoreServices/CoreServices.h>
#import <Foundation/Foundation.h>
#include <stdint.h>

// Rust 側の受け口（device/file_watch.rs）。kind 0 = 変化の合図、それ以外 = 失敗。
extern void fandhe_rust_file_watch_fire(uint64_t id, int32_t kind);

@interface FandheFsWatch : NSObject
@property(nonatomic) FSEventStreamRef stream;
@property(nonatomic, strong) dispatch_queue_t queue;
@end
@implementation FandheFsWatch
@end

static NSMutableDictionary<NSNumber *, FandheFsWatch *> *g_watches;

static NSMutableDictionary<NSNumber *, FandheFsWatch *> *watches(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        g_watches = [NSMutableDictionary dictionary];
    });
    return g_watches;
}

static void fandhe_fs_callback(ConstFSEventStreamRef stream, void *info, size_t count,
                               void *paths, const FSEventStreamEventFlags flags[],
                               const FSEventStreamEventId ids[]) {
    (void)stream; (void)paths; (void)ids;
    // 通常のイベントの中身は読まない。ただし監視ルート自体の移動・削除（RootChanged）は、
    // ストリームが元のパスを追わなくなるため失敗（kind 1）として止める。
    for (size_t i = 0; i < count; i++) {
        if (flags[i] & kFSEventStreamEventFlagRootChanged) {
            fandhe_rust_file_watch_fire((uint64_t)(uintptr_t)info, 1);
            return;
        }
    }
    fandhe_rust_file_watch_fire((uint64_t)(uintptr_t)info, 0);
}

int32_t fandhe_macos_file_watch_start(uint64_t id, const char *path) {
    if (path == NULL) {
        return 1;
    }
    @autoreleasepool {
        NSString *p = [NSString stringWithUTF8String:path];
        if (p == nil) {
            return 2;
        }
        NSMutableDictionary *map = watches();
        @synchronized(map) {
            if (map[@(id)] != nil) {
                return 3;
            }
        }
        FSEventStreamContext ctx = {0, (void *)(uintptr_t)id, NULL, NULL, NULL};
        FSEventStreamRef stream = FSEventStreamCreate(
            NULL, fandhe_fs_callback, &ctx, (__bridge CFArrayRef) @[ p ],
            kFSEventStreamEventIdSinceNow, 0.2,
            kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagWatchRoot);
        if (stream == NULL) {
            return 4;
        }
        dispatch_queue_t queue = dispatch_queue_create("com.fandhe.file-watch", DISPATCH_QUEUE_SERIAL);
        FSEventStreamSetDispatchQueue(stream, queue);
        if (!FSEventStreamStart(stream)) {
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
            return 5;
        }
        FandheFsWatch *w = [FandheFsWatch new];
        w.stream = stream;
        w.queue = queue;
        @synchronized(map) {
            map[@(id)] = w;
        }
        return 0;
    }
}

void fandhe_macos_file_watch_stop(uint64_t id) {
    @autoreleasepool {
        NSMutableDictionary *map = watches();
        FandheFsWatch *w = nil;
        @synchronized(map) {
            w = map[@(id)];
            [map removeObjectForKey:@(id)];
        }
        if (w == nil) {
            return;
        }
        FSEventStreamStop(w.stream);
        FSEventStreamInvalidate(w.stream);
        // 実行中のコールバックの終了を待つ（コールバックは専用 queue 上でだけ走る）。
        dispatch_sync(w.queue, ^{
                      });
        FSEventStreamRelease(w.stream);
    }
}
