// Phase 11 の拡充ループ（親 #174）型サンプル「Dock・タスクバーの進捗表示と注意喚起」: macOS 実装（Issue #860）。
// `NSDockTile.contentView` にアイコン＋進捗バーを自前で描いて `display` し、注意喚起は
// `requestUserAttention:NSCriticalRequest`（前面になるか取り消されるまで Dock のアイコンが跳ねる）。権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/macos_app_progress_attention.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約（3 関数とも UI スレッド（main thread）から同期で呼ぶ）:
//   fandhe_macos_app_progress_set(uint32_t percent) -> int32_t
//   fandhe_macos_app_progress_clear(void) -> int32_t
//   fandhe_macos_app_attention_request(void) -> int32_t
//     0 = OS へ渡した / 1 = NSApp または dockTile が無い / 2 = main thread ではない
//     percent の範囲（0〜100）の検証は Rust 側で済んでいる（念のためここでも 100 に丸めず 1 を返す）。
//     requestUserAttention の戻り値（取り消し用 id）は使わない。
//   contentView はタイル全体を自前で描く（バッジは OS が上に重ねるので描かない）。view は組み立て終えてから
//   差し替え、途中で失敗したら既存の contentView を触らない。Cocoa は自動で描き直さないので変更のたびに
//   `display` を送る。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは受け取らず保持しない。ログは出さない。

#import <AppKit/AppKit.h>
#include <stdint.h>

/// アプリのアイコンの下部に進捗バー（背景＋塗り）を重ねた画像を作る。
static NSImage *fandhe_progress_tile_image(NSSize size, uint32_t percent) {
    NSImage *image = [[NSImage alloc] initWithSize:size];
    [image lockFocus];
    [NSApp.applicationIconImage drawInRect:NSMakeRect(0, 0, size.width, size.height)
                                  fromRect:NSZeroRect
                                 operation:NSCompositingOperationSourceOver
                                  fraction:1.0];
    CGFloat margin = size.width * 0.1;
    NSRect track = NSMakeRect(margin, size.height * 0.08, size.width - margin * 2, size.height * 0.1);
    CGFloat radius = track.size.height / 2;
    [[NSColor colorWithWhite:0.0 alpha:0.55] setFill];
    [[NSBezierPath bezierPathWithRoundedRect:track xRadius:radius yRadius:radius] fill];
    NSRect fill = track;
    fill.size.width = track.size.width * ((CGFloat)percent / 100.0);
    if (fill.size.width > 0) {
        [[NSColor systemBlueColor] setFill];
        [[NSBezierPath bezierPathWithRoundedRect:fill xRadius:radius yRadius:radius] fill];
    }
    [image unlockFocus];
    return image;
}

int32_t fandhe_macos_app_progress_set(uint32_t percent) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
        NSDockTile *tile = NSApp.dockTile;
        if (NSApp == nil || tile == nil || percent > 100) {
            return 1;
        }
        NSImage *image = fandhe_progress_tile_image(tile.size, percent);
        NSImageView *view = [[NSImageView alloc] initWithFrame:NSMakeRect(0, 0, tile.size.width, tile.size.height)];
        view.image = image;
        tile.contentView = view;
        [tile display];
        return 0;
    }
}

int32_t fandhe_macos_app_progress_clear(void) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
        NSDockTile *tile = NSApp.dockTile;
        if (NSApp == nil || tile == nil) {
            return 1;
        }
        tile.contentView = nil;
        [tile display];
        return 0;
    }
}

int32_t fandhe_macos_app_attention_request(void) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
        if (NSApp == nil) {
            return 1;
        }
        // 戻り値は取り消し用の id。結果には使わない（OS を正にする）。
        (void)[NSApp requestUserAttention:NSCriticalRequest];
        return 0;
    }
}
