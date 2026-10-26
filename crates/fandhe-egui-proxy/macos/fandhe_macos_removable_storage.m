// Phase 11 の拡充ループ（親 #175）型サンプル「外部ストレージの着脱」: macOS 実装（Issue #858）。
// NSWorkspace の通知センターでボリュームのマウント・取り外しの直前・取り外しの 3 通知を受けて出来事の
// コードだけを Rust へ返し、今のボリューム一覧は NSFileManager.mountedVolumeURLs で読む。
// 通知の userInfo（NSDevicePath）は読まず、URL・パス・UUID は Rust へ渡さない（フィンガープリント防止）。
// 権限の宣言は増やさない（本体アプリは sandbox なし）。
//
// `crates/fandhe-egui-proxy/src/device/macos_removable_storage.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_removable_storage_start(id) -> int32_t
//     3 通知の observer を登録する。0 = 成功、非 0 = 失敗（登録済みの分を全て外し、何も残さない）。
//     届くたびに fandhe_rust_removable_storage_fire(sub_id, code) を呼ぶ。code: 0 = DidMount、
//     1 = WillUnmount、2 = DidUnmount。ブロックは呼ぶだけですぐ戻る。
//     OS 資源は id で所有し、生ポインタを Rust へ渡さない。同じ id の二重登録は失敗。
//     呼び出しスレッドは任意（NSNotificationCenter はスレッドセーフ）。main へ dispatch_sync しない。
//   fandhe_macos_removable_storage_stop(id)
//     observer を全て外して解放する。未知の id は何もしない。stop の後に新しい通知は配送されない
//     （既に実行中のブロックは短く、Rust 側の登録表で捨てられる）。
//   fandhe_macos_removable_storage_list(FandheVolumesRaw *out) -> int32_t
//     隠しボリュームを除くマウント済みボリュームを最大 32 件、out へ書く。0 = 成功、非 0 = 失敗
//     （mountedVolumeURLs が nil）。読めない属性は -1（0 = false、1 = true）。名前は NUL 終端の UTF-8
//     で最大 255 バイト（文字境界で切る）。I/O が要るボリュームでは待つことがある（守らない範囲）。
//
// FFI 規約: 各関数を `@autoreleasepool` で囲む。辞書の操作は @synchronized で守り、ロックの内側では待たない。

#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#include <stdint.h>
#include <string.h>

// Rust 側の受け口（device/removable_storage.rs）。
extern void fandhe_rust_removable_storage_fire(uint64_t sub_id, int32_t code);

#define FANDHE_MAX_VOLUMES 32
#define FANDHE_NAME_BYTES 256

// Rust 側の `#[repr(C)] struct Raw` / `RawItem` と同じレイアウト（全フィールドが 1 バイト境界か i32 先頭）。
typedef struct {
    char name[FANDHE_NAME_BYTES];
    int8_t internal;
    int8_t removable;
    int8_t ejectable;
    int8_t local;
} FandheVolumeItem;

typedef struct {
    int32_t count;
    FandheVolumeItem items[FANDHE_MAX_VOLUMES];
} FandheVolumesRaw;

static NSMutableDictionary<NSNumber *, NSArray *> *g_observers;

static NSMutableDictionary<NSNumber *, NSArray *> *observers(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        g_observers = [NSMutableDictionary dictionary];
    });
    return g_observers;
}

int32_t fandhe_macos_removable_storage_start(uint64_t sub_id) {
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
            NSWorkspaceDidMountNotification,
            NSWorkspaceWillUnmountNotification,
            NSWorkspaceDidUnmountNotification,
        ];
        NSMutableArray *tokens = [NSMutableArray arrayWithCapacity:names.count];
        for (NSUInteger i = 0; i < names.count; i++) {
            int32_t code = (int32_t)i;
            id token = [center addObserverForName:names[i]
                                           object:nil
                                            queue:nil
                                       usingBlock:^(NSNotification *note) {
                                           // userInfo（NSDevicePath）は読まない。
                                           (void)note;
                                           fandhe_rust_removable_storage_fire(sub_id, code);
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

void fandhe_macos_removable_storage_stop(uint64_t sub_id) {
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

// URLResourceKey の真偽値を 1 / 0 / -1（読めない）で返す。
static int8_t read_flag(NSURL *url, NSURLResourceKey key) {
    NSNumber *value = nil;
    NSError *error = nil;
    if (![url getResourceValue:&value forKey:key error:&error] || ![value isKindOfClass:[NSNumber class]]) {
        return -1;
    }
    return value.boolValue ? 1 : 0;
}

int32_t fandhe_macos_removable_storage_list(FandheVolumesRaw *out) {
    @autoreleasepool {
        if (out == NULL) {
            return 1;
        }
        memset(out, 0, sizeof(*out));
        NSArray<NSURLResourceKey> *keys = @[
            NSURLVolumeLocalizedNameKey,
            NSURLVolumeIsRemovableKey,
            NSURLVolumeIsEjectableKey,
            NSURLVolumeIsInternalKey,
            NSURLVolumeIsLocalKey,
        ];
        NSArray<NSURL *> *urls = [[NSFileManager defaultManager]
            mountedVolumeURLsIncludingResourceValuesForKeys:keys
                                                    options:NSVolumeEnumerationSkipHiddenVolumes];
        if (urls == nil) {
            return 2;
        }
        int32_t n = 0;
        for (NSURL *url in urls) {
            if (n >= FANDHE_MAX_VOLUMES) {
                break;
            }
            FandheVolumeItem *item = &out->items[n];
            NSString *name = nil;
            NSError *error = nil;
            if ([url getResourceValue:&name forKey:NSURLVolumeLocalizedNameKey error:&error] &&
                [name isKindOfClass:[NSString class]]) {
                // 最大 255 バイト + NUL。文字境界で切る（途中で切れる文字は入れない）。
                NSUInteger used = 0;
                [name getBytes:item->name
                     maxLength:FANDHE_NAME_BYTES - 1
                    usedLength:&used
                      encoding:NSUTF8StringEncoding
                       options:0
                         range:NSMakeRange(0, name.length)
                remainingRange:NULL];
                item->name[used] = '\0';
            }
            item->internal = read_flag(url, NSURLVolumeIsInternalKey);
            item->removable = read_flag(url, NSURLVolumeIsRemovableKey);
            item->ejectable = read_flag(url, NSURLVolumeIsEjectableKey);
            item->local = read_flag(url, NSURLVolumeIsLocalKey);
            n++;
        }
        out->count = n;
        return 0;
    }
}
