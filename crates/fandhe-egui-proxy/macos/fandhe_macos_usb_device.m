// Phase 11 の拡充ループ（親 #112）型サンプル「USB 機器」: macOS 実装（Issue #505）。
// IOKit の `IOUSBHostDevice` を列挙し、ベンダー ID・製品 ID・製品名を同期で読む。機器は開かない
// ので TCC も entitlement も増やさない。シリアル番号・locationID など識別子は読まない。
//
// `crates/fandhe-egui-proxy/src/device/macos_usb_device.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし IOKit をリンクする。
//
// 契約:
//   fandhe_macos_usb_device_list(FandheUsbDeviceListRaw *out) -> int32_t
//     UI スレッドから同期で呼ぶ。0 = 成功、非 0 = 失敗（out は無効）。
//     count は 0..=64（64 件に達したらそれ以上は書かない）。VID/PID が取れない項目は書かない。
//     名前は NUL 終端の UTF-8（最大 511 バイト、文字境界で切る）。取れなければ空文字。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。service・iterator・
// プロパティは全経路で解放する。

#import <Foundation/Foundation.h>
#include <IOKit/IOKitLib.h>
#include <stdint.h>
#include <string.h>

#define FANDHE_USB_MAX 64
#define FANDHE_USB_NAME_BYTES 512

typedef struct {
    int32_t vendor_id;
    int32_t product_id;
    char name[FANDHE_USB_NAME_BYTES];
} FandheUsbDeviceRaw;

typedef struct {
    int32_t count;
    FandheUsbDeviceRaw entries[FANDHE_USB_MAX];
} FandheUsbDeviceListRaw;

// 数値プロパティを読む。取れれば 1。
static int fandhe_usb_number(io_service_t svc, CFStringRef key, int32_t *value) {
    CFTypeRef p = IORegistryEntryCreateCFProperty(svc, key, kCFAllocatorDefault, 0);
    if (p == NULL) {
        return 0;
    }
    int ok = CFGetTypeID(p) == CFNumberGetTypeID() &&
             CFNumberGetValue((CFNumberRef)p, kCFNumberSInt32Type, value);
    CFRelease(p);
    return ok;
}

// 製品名を dst（FANDHE_USB_NAME_BYTES）へ書く。取れなければ空文字。
static void fandhe_usb_name(io_service_t svc, char *dst) {
    dst[0] = '\0';
    CFTypeRef p = IORegistryEntryCreateCFProperty(svc, CFSTR("USB Product Name"),
                                                  kCFAllocatorDefault, 0);
    if (p == NULL) {
        return;
    }
    if (CFGetTypeID(p) == CFStringGetTypeID()) {
        NSString *s = (__bridge NSString *)p;
        NSUInteger used = 0;
        if ([s getBytes:dst
                 maxLength:FANDHE_USB_NAME_BYTES - 1
                usedLength:&used
                  encoding:NSUTF8StringEncoding
                   options:0
                     range:NSMakeRange(0, s.length)
            remainingRange:NULL]) {
            dst[used] = '\0';
        } else {
            dst[0] = '\0';
        }
    }
    CFRelease(p);
}

int32_t fandhe_macos_usb_device_list(FandheUsbDeviceListRaw *out) {
    if (out == NULL) {
        return 1;
    }
    @autoreleasepool {
        out->count = 0;
        io_iterator_t iter = IO_OBJECT_NULL;
        // matching 辞書はこの呼び出しが consume するので CFRelease しない。
        if (IOServiceGetMatchingServices(0 /* 既定 port */, IOServiceMatching("IOUSBHostDevice"),
                                         &iter) != KERN_SUCCESS) {
            return 2;
        }
        io_service_t svc;
        // 64 件に達しても iterator は最後まで回して service を解放する。
        while ((svc = IOIteratorNext(iter)) != IO_OBJECT_NULL) {
            if (out->count < FANDHE_USB_MAX) {
                FandheUsbDeviceRaw *e = &out->entries[out->count];
                if (fandhe_usb_number(svc, CFSTR("idVendor"), &e->vendor_id) &&
                    fandhe_usb_number(svc, CFSTR("idProduct"), &e->product_id)) {
                    fandhe_usb_name(svc, e->name);
                    out->count++;
                }
            }
            IOObjectRelease(svc);
        }
        IOObjectRelease(iter);
    }
    return 0;
}
