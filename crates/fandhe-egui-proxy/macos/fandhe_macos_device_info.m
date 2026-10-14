// Phase 11 の拡充ループ（親 #1131）型サンプル「端末・OS 情報」: macOS 実装（Issue #1304）。
// `NSProcessInfo` で OS の版と物理メモリを、`sysctlbyname("hw.model")` で機種の型番を、
// `NSURLVolumeAvailableCapacityForImportantUsageKey` でホームのボリュームの空き容量（OS の
// 見積り）を同期で読む。個体識別子（`hw.uuid`・シリアル）は読まない。
//
// `crates/fandhe-egui-proxy/src/device/macos_device_info.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_device_info_read(FandheDeviceInfoRaw *out) -> int32_t
//     UI スレッドから同期で呼ぶ（ブロックしない前提）。0 = 成功、非 0 = 失敗（out は無効）。
//     out の各フィールドは「取れない」を番兵で表す（Rust 側が範囲外も None へ落とす）:
//       os_major / os_minor / os_patch: -1 不明 / 0 以上
//       memory_bytes: -1 不明 / 0 以上
//       free_bytes:   -1 不明 / 0 以上（0 は有効値）
//       model: NUL 終端の UTF-8。取れなければ空文字
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <Foundation/Foundation.h>
#include <stdint.h>
#include <string.h>
#include <sys/sysctl.h>

typedef struct {
    int64_t os_major;
    int64_t os_minor;
    int64_t os_patch;
    int64_t memory_bytes;
    int64_t free_bytes;
    char model[64];
} FandheDeviceInfoRaw;

int32_t fandhe_macos_device_info_read(FandheDeviceInfoRaw *out) {
    if (out == NULL) {
        return 1;
    }
    @autoreleasepool {
        out->os_major = -1;
        out->os_minor = -1;
        out->os_patch = -1;
        out->memory_bytes = -1;
        out->free_bytes = -1;
        memset(out->model, 0, sizeof(out->model));

        NSProcessInfo *info = [NSProcessInfo processInfo];
        NSOperatingSystemVersion v = info.operatingSystemVersion;
        out->os_major = (int64_t)v.majorVersion;
        out->os_minor = (int64_t)v.minorVersion;
        out->os_patch = (int64_t)v.patchVersion;
        if (info.physicalMemory <= (unsigned long long)INT64_MAX) {
            out->memory_bytes = (int64_t)info.physicalMemory;
        }

        // 収まらない・失敗のときは空のまま（切り詰めた型番を渡さない）。
        char buf[64];
        size_t len = sizeof(buf);
        if (sysctlbyname("hw.model", buf, &len, NULL, 0) == 0 && len > 0 && len < sizeof(buf)) {
            buf[len] = '\0';
            memcpy(out->model, buf, len + 1);
        }

        NSNumber *free = nil;
        NSError *error = nil;
        NSURL *home = [NSURL fileURLWithPath:NSHomeDirectory()];
        if ([home getResourceValue:&free
                            forKey:NSURLVolumeAvailableCapacityForImportantUsageKey
                             error:&error] &&
            free != nil && free.longLongValue >= 0) {
            out->free_bytes = free.longLongValue;
        }
    }
    return 0;
}
