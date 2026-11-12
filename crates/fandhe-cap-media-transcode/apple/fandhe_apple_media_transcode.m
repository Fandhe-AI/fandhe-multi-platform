// Phase 11 の拡充ループ（親 #91）型サンプル「動画・音声の変換」: macOS 実装（Issue #311）。
// iOS 実装（b-ios の子 issue）と共用できるよう `TARGET_OS` では分けない（AVFoundation と
// Foundation だけを使い、UIKit / AppKit に触れない）。
//
// `crates/fandhe-egui-proxy/src/device/apple_media_transcode.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS・iOS ターゲットの feature `device` 有効時に `cc` コンパイルする
// （Rust 側 cfg と同じ OS 集合に保つ）。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_apple_media_transcode_run(const int16_t *pcm, size_t n, double sample_rate, void *ctx,
//                                    void (*on_output)(void *ctx, const uint8_t *bytes, size_t len))
//       -> int32_t
//     pcm/n        : モノラル PCM 16bit とそのサンプル数。呼び出しの間だけ参照し保持しない
//     sample_rate  : サンプルレート（Hz）
//     on_output    : 成功時に 1 回だけ、戻る前に同期で呼ぶ。`ctx` と `bytes` は呼び出しの間だけ有効。
//                    この関数は `ctx` / `bytes` を保持しない
//     0 = 成功
//     1 = 入力不正（n == 0・sample_rate が正でない・n が大きすぎる）
//     2 = 書き込みの失敗（AVAudioFile の作成・バッファ確保・書き込み）
//     3 = 読み戻しの失敗（書いた m4a を読めない。data == nil）
//     4 = 出力が 0 バイト（読めたが空。Rust 側は EMPTY に対応付ける）
//
// 出力は AAC-LC の m4a。一時ファイル `NSTemporaryDirectory()/fandhe-transcode-<UUID>.m4a` に書き、
// `AVAudioFile` を解放してファイルを確定させてから読み戻す。**成功・失敗・早期 return の全経路で
// 一時ファイルを消す**（作成に失敗してもファイルが残ることがあるため、そこでも消す）。
// 期限は Rust のドライバだけが持つ。この関数は中断できず走り切る（結果はドライバが捨てる）。
//
// スレッド: 呼ばれたスレッド（Rust のワーカー）で同期に実行する（`dispatch_sync` は使わない。
// main 側が呼び出し元を待つとデッドロックするため、`device-bridge-contracts.md`「11」）。
// Cocoa 呼び出しは `@autoreleasepool` の内側で行う。
//
// 音声・NSError の文面はログへ出さない（security.md A09）。権限・entitlement は不要。

#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>

// 書き込みと読み戻しを行い、成功ならコールバックを 1 回呼ぶ。一時ファイルの削除は呼び出し側。
static int32_t fandhe_transcode_write_read(NSURL *url, const int16_t *pcm, size_t n,
                                           double sample_rate, void *ctx,
                                           void (*on_output)(void *ctx, const uint8_t *bytes,
                                                             size_t len)) {
    @autoreleasepool {
        AVAudioFile *file = nil;
        @autoreleasepool {
            NSDictionary<NSString *, id> *settings = @{
                AVFormatIDKey : @(kAudioFormatMPEG4AAC),
                AVSampleRateKey : @(sample_rate),
                AVNumberOfChannelsKey : @1,
            };
            NSError *error = nil;
            file = [[AVAudioFile alloc] initForWriting:url
                                              settings:settings
                                          commonFormat:AVAudioPCMFormatInt16
                                           interleaved:YES
                                                 error:&error];
            if (file == nil) {
                return 2;
            }
            AVAudioPCMBuffer *buffer =
                [[AVAudioPCMBuffer alloc] initWithPCMFormat:file.processingFormat
                                              frameCapacity:(AVAudioFrameCount)n];
            if (buffer == nil || buffer.int16ChannelData == NULL) {
                return 2;
            }
            memcpy(buffer.int16ChannelData[0], pcm, n * sizeof(int16_t));
            buffer.frameLength = (AVAudioFrameCount)n;
            if (![file writeFromBuffer:buffer error:&error]) {
                return 2;
            }
            // 公開の close は iOS 17 下限では当てにせず、解放でファイルを確定させる。
            file = nil;
        }
        NSData *data = [NSData dataWithContentsOfURL:url];
        if (data == nil) {
            return 3;
        }
        if (data.length == 0) {
            return 4;
        }
        on_output(ctx, (const uint8_t *)data.bytes, data.length);
        return 0;
    }
}

int32_t fandhe_apple_media_transcode_run(const int16_t *pcm, size_t n, double sample_rate,
                                         void *ctx,
                                         void (*on_output)(void *ctx, const uint8_t *bytes,
                                                           size_t len)) {
    @autoreleasepool {
        if (pcm == NULL || n == 0 || n > UINT32_MAX || !(sample_rate > 0)) {
            return 1;
        }
        NSString *name =
            [NSString stringWithFormat:@"fandhe-transcode-%@.m4a", [NSUUID UUID].UUIDString];
        NSURL *url = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:name]];
        int32_t code = fandhe_transcode_write_read(url, pcm, n, sample_rate, ctx, on_output);
        // 全経路で消す。`file` はヘルパー内で解放済み。
        [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
        return code;
    }
}
