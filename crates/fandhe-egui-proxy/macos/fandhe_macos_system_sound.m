// Phase 11 の拡充ループ（親 #183）型サンプル「システムの効果音・ビープ」: macOS 実装（Issue #939）。
// `NSBeep()` で OS 標準の警告音を 1 回鳴らす。権限は不要。音は利用者がシステム設定のサウンドで選ぶ。
//
// `crates/fandhe-egui-proxy/src/device/macos_system_sound.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_system_sound_beep(void) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。
//     0 = OS に渡した（NSBeep は戻り値がなく、鳴ったかは検知できない）
//     2 = main thread ではない（AppKit に触らず返す。dispatch_sync は使わない）
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは受け取らず保持しない。ログは出さない。

#import <AppKit/AppKit.h>
#include <stdint.h>

int32_t fandhe_macos_system_sound_beep(void) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
        NSBeep();
        return 0;
    }
}
