// アプリ・端末の識別子: iOS 実装（Issue #904、親 #181）。`UIDevice.currentDevice.identifierForVendor`
// （IDFV）を同期で 1 回読み、NUL 終端の UTF-8 として呼び出し元のバッファへ書く。
// 広告 ID（IDFA）は読まず、権限・usage description・privacy manifest は要らない。
//
// `src/device/ios_device_identifier.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時にのみコンパイルする（Foundation・UIKit はリンク済み）。
//
// 契約:
//   fandhe_ios_device_identifier_read(char *out, size_t cap) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。戻り値:
//       0 成功。NUL 終端の UTF-8 を out に書いた（空文字でも 0。Rust 側が Internal へ落とす）
//       1 out が NULL、または cap が 0
//       2 main thread 以外から呼ばれた（UIKit に触らず、dispatch_sync もしない）
//       3 identifierForVendor が nil（再起動後・初回ロック解除前など。失敗ではなく「後で取り直す」）
//       4 UTF-8 化に失敗、または cap に収まらない（切り詰めた値を成功として返さない）
//
// FFI 規約: UIKit に触る部分は `@autoreleasepool` で囲む。ポインタは保持しない。ログは出さない。
// OS コールバックを持たないため、再描画の要求（`request_repaint`）も不要。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#include <stdint.h>
#include <string.h>

int32_t fandhe_ios_device_identifier_read(char *out, size_t cap) {
    if (out == NULL || cap == 0) {
        return 1;
    }
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        NSUUID *uuid = UIDevice.currentDevice.identifierForVendor;
        if (uuid == nil) {
            return 3;
        }
        const char *s = uuid.UUIDString.UTF8String;
        if (s == NULL) {
            return 4;
        }
        size_t n = strlen(s);
        if (n + 1 > cap) {
            return 4;
        }
        memcpy(out, s, n + 1);
    }
    return 0;
}
