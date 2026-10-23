// Phase 11 の拡充ループ（親 #125）型サンプル「無操作時間の検知」: macOS 実装（Issue #631）。
// CoreGraphics の `CGEventSourceSecondsSinceLastEventType` で、ログインセッション全体の
// 「最後の入力からの経過秒」を同期で読む（公式文書に権限の記載なし）。
//
// `crates/fandhe-egui-proxy/src/device/macos_user_idle.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし、CoreGraphics を
// リンクする。
//
// 契約:
//   double fandhe_macos_user_idle_seconds(void)
//     UI スレッドから同期で呼ぶ（ブロックしない）。OS の戻り値をそのまま返す。
//     NaN・負などの不正値の検証は Rust 側（`fandhe_device::user_idle::idle_from_seconds`）が行い、
//     ここで 0 などへ丸めない。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。

#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>

double fandhe_macos_user_idle_seconds(void) {
    @autoreleasepool {
        return CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateCombinedSessionState,
                                                      kCGAnyInputEventType);
    }
}
