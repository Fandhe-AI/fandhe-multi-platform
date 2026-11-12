// Phase 11 の拡充ループ（親 #1141）型サンプル「バッジの設定」: macOS 実装（Issue #1360）。
// `NSDockTile.badgeLabel` に件数の文字列を設定し、0 なら nil で消す。権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/macos_app_badge.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_app_badge_set(uint32_t value) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。
//     0 = OS へ渡した / 1 = NSApp または dockTile が無い / 2 = main thread ではない
//     value 0 は消去（badgeLabel = nil）。範囲（0〜9999）の検証は Rust 側で済んでいる。
//     未バンドル実行（cargo run）で Dock に出るかは #1359 の確認項目。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは受け取らず保持しない。ログは出さない。

#import <AppKit/AppKit.h>
#include <stdint.h>

int32_t fandhe_macos_app_badge_set(uint32_t value) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
        NSDockTile *tile = NSApp.dockTile;
        if (NSApp == nil || tile == nil) {
            return 1;
        }
        tile.badgeLabel = value == 0 ? nil : [NSString stringWithFormat:@"%u", value];
        return 0;
    }
}
