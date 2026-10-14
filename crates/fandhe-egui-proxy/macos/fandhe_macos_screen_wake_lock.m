// Phase 11 の拡充ループ（親 #1129）型サンプル「画面スリープの抑止」: macOS 実装（Issue #1278）。
// IOKit の電源アサーション（`kIOPMAssertionTypePreventUserIdleDisplaySleep`）を取る / 外す。
// macOS 専用 API のため `apple/*.m` ではなく `macos/` に置く。
//
// `crates/fandhe-egui-proxy/src/device/macos_screen_wake_lock.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし、IOKit と
// CoreFoundation をリンクする。
//
// 契約:
//   fandhe_macos_screen_wake_lock_acquire(uint32_t *out_id) -> int32_t
//     0 = アサーションを作成し `*out_id` に ID を書いた。1 = 作成に失敗（`*out_id` は不変）。
//     2 = `out_id` が NULL。二重取得の抑止（取得済みなら呼ばない）は Rust 側が `Mutex` で行う。
//   fandhe_macos_screen_wake_lock_release(uint32_t id) -> int32_t
//     0 = 解放した。1 = 失敗（アサーションは残っているとみなす）。
//
// スレッド契約: IOKit のアサーション API にスレッド制約は無いが、呼び出し側は UI スレッドから
// 呼ぶ（trait doc の契約）。呼び出しは短く同期で完了し、Cocoa オブジェクトは作らない。
// アサーションはプロセス終了で OS が解放するため、終了時の後片付けは持たない。

#import <Foundation/Foundation.h>
#import <IOKit/pwr_mgt/IOPMLib.h>

int32_t fandhe_macos_screen_wake_lock_acquire(uint32_t *out_id) {
    if (out_id == NULL) {
        return 2;
    }
    IOPMAssertionID assertion_id = kIOPMNullAssertionID;
    IOReturn rc = IOPMAssertionCreateWithName(
        kIOPMAssertionTypePreventUserIdleDisplaySleep, kIOPMAssertionLevelOn,
        CFSTR("fandhe: screen wake lock"), &assertion_id);
    if (rc != kIOReturnSuccess) {
        return 1;
    }
    *out_id = (uint32_t)assertion_id;
    return 0;
}

int32_t fandhe_macos_screen_wake_lock_release(uint32_t id) {
    return IOPMAssertionRelease((IOPMAssertionID)id) == kIOReturnSuccess ? 0 : 1;
}
