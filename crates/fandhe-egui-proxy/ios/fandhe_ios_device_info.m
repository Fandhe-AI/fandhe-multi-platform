// 端末・OS 情報: iOS 実装（Issue #1305）。`NSProcessInfo` で OS の版と物理メモリを、
// `sysctlbyname("hw.machine")` で機種の型番（`iPhone16,1` 等）を、
// `NSURLVolumeAvailableCapacityForImportantUsageKey` でホームのボリュームの空き容量
// （OS の見積り）を同期で読む。Foundation のみで UIKit に触れないため main スレッド制約は無い。
// 個体識別子（UDID・`identifierForVendor`・シリアル）と端末名（`UIDevice.name`）は読まない。
//
// `src/device/ios_device_info.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時にのみコンパイルする（Foundation はリンク済み）。
//
// 契約:
//   fandhe_ios_device_info_read(FandheIosDeviceInfoRaw *out) -> int32_t
//     UI スレッドから同期で呼ぶ（ブロックしない前提）。0 = 成功、1 = out が NULL。
//     out の各フィールドは「取れない」を番兵で表す（Rust 側が範囲外も None へ落とす）:
//       os_major / os_minor / os_patch: -1 不明 / 0 以上
//       memory_bytes: -1 不明 / 0 以上（0 は Rust 側で不明扱い）
//       free_bytes:   -1 不明 / 0 以上（0 は有効値）
//       model: NUL 終端の UTF-8。取れなければ空文字
//     シミュレータの `hw.machine` はホストの `arm64` を返し機種として嘘になるため、
//     シミュレータでは環境変数 SIMULATOR_MODEL_IDENTIFIER を機種とする（無ければ空）。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。ログは出さない。

#import <Foundation/Foundation.h>
#include <TargetConditionals.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/sysctl.h>

// Rust 側 `Raw`（#[repr(C)]）とフィールドの順・型を揃える。
typedef struct {
    int64_t os_major;
    int64_t os_minor;
    int64_t os_patch;
    int64_t memory_bytes;
    int64_t free_bytes;
    char model[64];
} FandheIosDeviceInfoRaw;

int32_t fandhe_ios_device_info_read(FandheIosDeviceInfoRaw *out) {
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
#if TARGET_OS_SIMULATOR
        const char *sim = getenv("SIMULATOR_MODEL_IDENTIFIER");
        if (sim != NULL) {
            size_t n = strlen(sim);
            if (n > 0 && n < sizeof(out->model)) {
                memcpy(out->model, sim, n + 1);
            }
        }
#else
        char buf[64];
        size_t len = sizeof(buf);
        if (sysctlbyname("hw.machine", buf, &len, NULL, 0) == 0 && len > 0 && len < sizeof(buf)) {
            buf[len] = '\0';
            memcpy(out->model, buf, len + 1);
        }
#endif

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
