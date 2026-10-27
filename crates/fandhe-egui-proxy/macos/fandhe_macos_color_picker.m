// Phase 11 の拡充ループ（親 #184）: システムの色選択ダイアログ macOS 実装（Issue #941）。
// 共有の `NSColorPanel` を固定の初期色で開き、閉じた時点の色を sRGB に直して 0xRRGGBB の整数で Rust へ通知する。
//
// `crates/fandhe-egui-proxy/src/device/macos_color_picker.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする（AppKit は既存のリンク）。
//
// 契約（pull 型。`fandhe_macos_eye_dropper.m` と同型）:
//   int32_t fandhe_macos_color_picker_present(uint64_t request_id, int32_t initial_rgb)
//     main queue へ dispatch してすぐ 0 を返す。結果は非同期に下の関数で届く。
//   fandhe_rust_macos_color_picker_outcome(uint64_t request_id, int32_t code, int32_t rgb)
//     -> Rust 側（`macos_color_picker.rs`）が公開する `extern "C"` 関数。
//        code 表: 0 = 選択（rgb = 0xRRGGBB）、1 = 色を選ばずに閉じた、2 = sRGB 変換の失敗・成分が NaN、
//        3 = 提示できなかった（NSApp が無い）。code → 結果の写像の SSOT は Rust の
//        `fandhe_device::color_picker::result_from_apple_code`（`.m` では写像しない）。
//
// 確定: パネルに確定ボタンは無い。閉じる（NSWindowWillCloseNotification）までに
// NSColorPanelColorDidChangeNotification が 1 回以上来ていれば、閉じた時点の panel.color を結果とする。
// 来ていなければ「選ばずに閉じた」。初期色との比較では推測しない。初期色はプログラムで入れてから購読する
// （プログラムの設定による通知を「選んだ」と取り違えない）。
//
// スレッド: 生成・購読・表示・通知はすべて main queue。完了通知は 1 要求につき 1 回（`delivered`）。
// 閉じた通知の中で 2 つの購読トークンを必ず外す（解放漏れを作らない）。
//
// 守らない範囲: hidesOnDeactivate（既定 YES）で隠れた間の表示（閉じた通知は出ない）、閉じた通知が来ない場合の復旧。
// 共有パネルなので、別の経路で同じパネルが開かれている間の挙動も守らない。
//
// 情報非保持: 選んだ色はフィンガープリントに使えるため、`NSLog` を含むログ出力は一切行わない。
// ObjC の例外を Rust へ通さないため、例外を投げる API は使わない。

#import <AppKit/AppKit.h>
#include <math.h>
#include <stdint.h>

extern void fandhe_rust_macos_color_picker_outcome(uint64_t request_id, int32_t code, int32_t rgb);

// sRGB の外の色は各成分を 0〜1 に寄せてから丸める。NaN は -1。
static int32_t channel(CGFloat c) {
    if (isnan(c)) {
        return -1;
    }
    if (c < 0.0) c = 0.0;
    if (c > 1.0) c = 1.0;
    return (int32_t)lround(c * 255.0);
}

int32_t fandhe_macos_color_picker_present(uint64_t request_id, int32_t initial_rgb) {
    @autoreleasepool {
        dispatch_async(dispatch_get_main_queue(), ^{
            @autoreleasepool {
                if (NSApp == nil) {
                    fandhe_rust_macos_color_picker_outcome(request_id, 3, 0);
                    return;
                }
                NSColorPanel *panel = NSColorPanel.sharedColorPanel;
                panel.showsAlpha = NO;
                // 初期色を先に入れる。購読はその後。
                panel.color = [NSColor colorWithSRGBRed:((initial_rgb >> 16) & 0xFF) / 255.0
                                                  green:((initial_rgb >> 8) & 0xFF) / 255.0
                                                   blue:(initial_rgb & 0xFF) / 255.0
                                                  alpha:1.0];

                __block BOOL changed = NO;
                __block BOOL delivered = NO;
                __block id changeToken = nil;
                __block id closeToken = nil;
                NSNotificationCenter *center = NSNotificationCenter.defaultCenter;

                changeToken = [center addObserverForName:NSColorPanelColorDidChangeNotification
                                                  object:panel
                                                   queue:NSOperationQueue.mainQueue
                                              usingBlock:^(NSNotification *_Nonnull __unused note) {
                                                  changed = YES;
                                              }];
                closeToken = [center
                    addObserverForName:NSWindowWillCloseNotification
                                object:panel
                                 queue:NSOperationQueue.mainQueue
                            usingBlock:^(NSNotification *_Nonnull __unused note) {
                                // どの経路でも先に購読を外す。トークンは外した後に nil にして循環参照を切る。
                                [NSNotificationCenter.defaultCenter removeObserver:changeToken];
                                [NSNotificationCenter.defaultCenter removeObserver:closeToken];
                                changeToken = nil;
                                closeToken = nil;
                                if (delivered) {
                                    return;
                                }
                                delivered = YES;
                                if (!changed) {
                                    fandhe_rust_macos_color_picker_outcome(request_id, 1, 0);
                                    return;
                                }
                                NSColor *c = [panel.color colorUsingColorSpace:NSColorSpace.sRGBColorSpace];
                                int32_t r = c == nil ? -1 : channel(c.redComponent);
                                int32_t g = c == nil ? -1 : channel(c.greenComponent);
                                int32_t b = c == nil ? -1 : channel(c.blueComponent);
                                if (r < 0 || g < 0 || b < 0) {
                                    fandhe_rust_macos_color_picker_outcome(request_id, 2, 0);
                                } else {
                                    fandhe_rust_macos_color_picker_outcome(request_id, 0, (r << 16) | (g << 8) | b);
                                }
                            }];
                [panel orderFront:nil];
            }
        });
        return 0;
    }
}
