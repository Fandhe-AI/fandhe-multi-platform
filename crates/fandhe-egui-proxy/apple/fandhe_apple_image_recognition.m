// Phase 11 の拡充ループ（親 #1120）型サンプル「画像の文字認識」: macOS 実装（Issue #1250）。
// iOS 実装（#1252）と共用できるよう `TARGET_OS` では分けない（Vision と Foundation だけを使い、
// UIKit / AppKit に触れない）。
//
// `crates/fandhe-egui-proxy/src/device/apple_image_recognition.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS・iOS ターゲットの feature `device` 有効時に `cc` コンパイルする
// （Rust 側 cfg と同じ OS 集合に保つ）。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_apple_image_recognition_recognize(const uint8_t *bytes, size_t len, void *ctx,
//                                            void (*on_line)(void *ctx, const char *line)) -> int32_t
//     bytes/len: JPEG / PNG 等のバイト列（形式・8 MiB 上限・辺の上限は Rust 側で検査済み）。
//                復号は Vision（ImageIO）に任せる。コピーせず呼び出しの間だけ参照する
//     on_line  : 認識した行を Vision が返した順に 1 件ずつ同期で呼ぶ。`ctx` と `line` は
//                呼び出しの間だけ有効。この関数は `ctx` / `line` を保持しない
//     0 = 成功（0 行も成功 = 文字なし）
//     1 = 失敗（復号できない・`performRequests` の失敗・UTF-8 変換の失敗。呼び出し元が
//         `Internal` へ畳む）
//     2 = 認識言語（日本語）が端末で使えない（OS が古い等。呼び出し元が
//         `Unsupported(LANGUAGE_UNAVAILABLE)` へ写す）
//
// 空行の除去と前後空白の trim は Rust 側の `normalize_lines` が行う（契約の SSOT は
// `fandhe_device::image_recognition`）。
//
// スレッド: 呼び出しごとにリクエストとハンドラを作るため共有状態もロックも持たない。
// 呼ばれたスレッド（Rust のワーカー）で同期に実行する（`dispatch_sync` は使わない。main 側が
// 呼び出し元を待つとデッドロックするため、`device-bridge-contracts.md`「11」）。Cocoa 呼び出しは
// `@autoreleasepool` の内側で行う。
//
// 画像・認識した文字はログへ出さない（security.md A09）。権限・entitlement は不要。

#import <Foundation/Foundation.h>
#import <Vision/Vision.h>

int32_t fandhe_apple_image_recognition_recognize(const uint8_t *bytes, size_t len, void *ctx,
                                                 void (*on_line)(void *ctx, const char *line)) {
    @autoreleasepool {
        // 日本語の認識は revision 3（macOS 13 / iOS 16）以降。満たさなければ言語を使えない扱い。
        if (@available(macOS 13.0, iOS 16.0, *)) {
            NSData *data = [NSData dataWithBytesNoCopy:(void *)bytes length:len freeWhenDone:NO];
            VNRecognizeTextRequest *request = [[VNRecognizeTextRequest alloc] init];
            request.revision = VNRecognizeTextRequestRevision3;
            request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
            request.usesLanguageCorrection = YES;

            NSError *langError = nil;
            NSArray<NSString *> *supported =
                [request supportedRecognitionLanguagesAndReturnError:&langError];
            if (supported == nil || ![supported containsObject:@"ja-JP"]) {
                return 2;
            }
            request.recognitionLanguages = @[ @"ja-JP", @"en-US" ];

            VNImageRequestHandler *handler =
                [[VNImageRequestHandler alloc] initWithData:data options:@{}];
            NSError *error = nil;
            if (![handler performRequests:@[ request ] error:&error]) {
                return 1;
            }
            for (VNRecognizedTextObservation *observation in request.results) {
                VNRecognizedText *top = [observation topCandidates:1].firstObject;
                if (top == nil) {
                    continue;
                }
                const char *utf8 = [top.string UTF8String];
                if (utf8 == NULL) {
                    return 1;
                }
                on_line(ctx, utf8);
            }
            return 0;
        }
        return 2;
    }
}
