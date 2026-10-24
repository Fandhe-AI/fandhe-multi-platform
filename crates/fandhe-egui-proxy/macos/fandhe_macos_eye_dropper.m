// Phase 11 の拡充ループ（親 #148）型サンプル「画面のスポイト」: macOS 実装（Issue #762）。
// `NSColorSampler` で画面上の 1 点の色を拾い、sRGB に直して 0xRRGGBB の整数で Rust へ通知する。
//
// `crates/fandhe-egui-proxy/src/device/macos_eye_dropper.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする（AppKit は既存のリンク）。
//
// 契約（pull 型。`fandhe_apple_contact_picker.m` と同型）:
//   int32_t fandhe_macos_eye_dropper_present(uint64_t request_id)
//     main queue へ dispatch してすぐ 0 を返す。結果は非同期に下の関数で届く。
//   fandhe_rust_eye_dropper_outcome(uint64_t request_id, int32_t code, int32_t rgb)
//     -> Rust 側（`macos_eye_dropper.rs`）が公開する `extern "C"` 関数。
//        code 表: 0 = 選択（rgb = 0xRRGGBB）、1 = 取り消し（nil）、2 = sRGB 変換の失敗・成分が NaN、
//        3 = 起動できなかった（NSApp が無い）。code → 結果の写像の SSOT は Rust の
//        `fandhe_device::eye_dropper::result_from_macos_code`（`.m` では写像しない）。
//
// スレッド: サンプラーの生成・起動・ハンドラはすべて main queue（ハンドラは main thread で呼ばれる）。
// 完了通知は 1 要求につき 1 回（`delivered` で守る。Rust 側も RequestSlot で first-wins）。
// サンプラーはハンドラが強参照で保持し、通知後に nil にして解放する。
//
// 「開いているか」の static は持たない（処理中の再押下は画面がボタンを隠して防ぐ）。閉じる公開 API は無く
// 期限も持たない。ハンドラが呼ばれない場合の復旧は守らない範囲。
//
// 情報非保持: 拾った色はフィンガープリントに使えるため、`NSLog` を含むログ出力は一切行わない。
// ObjC の例外を Rust へ通さないため、例外を投げる API は使わない。

#import <AppKit/AppKit.h>
#include <math.h>
#include <stdint.h>

extern void fandhe_rust_eye_dropper_outcome(uint64_t request_id, int32_t code, int32_t rgb);

// sRGB の外の色は各成分を 0〜1 に寄せてから丸める。NaN は -1。
static int32_t channel(CGFloat c) {
    if (isnan(c)) {
        return -1;
    }
    if (c < 0.0) c = 0.0;
    if (c > 1.0) c = 1.0;
    return (int32_t)lround(c * 255.0);
}

int32_t fandhe_macos_eye_dropper_present(uint64_t request_id) {
    @autoreleasepool {
        dispatch_async(dispatch_get_main_queue(), ^{
            @autoreleasepool {
                if (NSApp == nil) {
                    fandhe_rust_eye_dropper_outcome(request_id, 3, 0);
                    return;
                }
                __block NSColorSampler *sampler = [[NSColorSampler alloc] init];
                __block BOOL delivered = NO;
                [sampler showSamplerWithSelectionHandler:^(NSColor *_Nullable color) {
                    if (!delivered) {
                        delivered = YES;
                        if (color == nil) {
                            fandhe_rust_eye_dropper_outcome(request_id, 1, 0);
                        } else {
                            NSColor *c = [color colorUsingColorSpace:NSColorSpace.sRGBColorSpace];
                            int32_t r = c == nil ? -1 : channel(c.redComponent);
                            int32_t g = c == nil ? -1 : channel(c.greenComponent);
                            int32_t b = c == nil ? -1 : channel(c.blueComponent);
                            if (r < 0 || g < 0 || b < 0) {
                                fandhe_rust_eye_dropper_outcome(request_id, 2, 0);
                            } else {
                                fandhe_rust_eye_dropper_outcome(request_id, 0, (r << 16) | (g << 8) | b);
                            }
                        }
                    }
                    sampler = nil;
                }];
            }
        });
        return 0;
    }
}
