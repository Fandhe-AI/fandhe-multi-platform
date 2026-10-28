// Phase 11 の拡充ループ（親 #340）型サンプル「動画の変換」: macOS 実装（Issue #1014）。
// iOS 実装（#1016）と共用できるよう `TARGET_OS` では分けない（AVFoundation・CoreMedia・CoreVideo・
// Foundation だけを使い、UIKit / AppKit に触れない）。
//
// `crates/fandhe-egui-proxy/src/device/apple_video_transcode.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS・iOS ターゲットの feature `device` 有効時に `cc` コンパイルする
// （Rust 側 cfg と同じ OS 集合に保つ）。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_apple_video_transcode_run(src_path, out_path, width, height, fps, frames, remaining_ns,
//                                    ctx, fill_frame) -> int32_t
//     src_path/out_path : 一時ファイルのパス（UTF-8）。Rust が作り、Rust が消す（この関数は消さない）。
//                         呼び出しの間だけ参照し保持しない
//     width/height/fps/frames : 元動画（H.264 の MP4）の寸法・フレームレート・フレーム数
//     remaining_ns      : 期限までの残り時間（ns）。Rust の `Instant` と同じ単調時計
//                         （CLOCK_UPTIME_RAW）で締切を作る
//     fill_frame        : 同期に呼ばれ、`base`（BGRA・`bytes_per_row` バイト/行・全体 `len` バイト）へ
//                         `index` 枚目の絵を描く。false なら失敗。`ctx` は呼び出しの間だけ有効で保持しない
//     0 = 成功（out_path に小さい動画が書かれた。中身の判定は Rust が読み直して行う）
//     1 = 入力不正（null path・寸法 / fps / frames が 0）
//     2 = 元動画の書き込みの失敗（writer の作成・追加・append・終了）
//     3 = 書き出しの失敗（export session の完了が Completed でない）
//     4 = 非対応（writer が設定を受け付けない / export session を作れない）
//     5 = 期限切れ（OS の操作は取り消し、完了ハンドラの到着まで確かめた）
//     6 = 期限切れ。取り消しても完了ハンドラが猶予内に届かなかった（OS の操作が残っている可能性。
//         Rust は一時ファイルを未解放として記録し、`fandhe_apple_video_transcode_settled` が真になるまで
//         次の呼び出しで OS を呼ばない）
//   fandhe_apple_video_transcode_settled() -> bool
//     前回の 6 の完了ハンドラが届いた（または 6 が無かった）なら真。
//
// 一時ファイルは Rust が持つ。ただし 6 以外では、戻る前に writer / export session の参照を手放す。
// 6 のときは完了ハンドラが writer / export session を保持して、完了まで解放しない。
//
// スレッド: 呼ばれたスレッド（Rust のワーカー）で同期に実行する（`dispatch_sync` は使わない。
// main 側が呼び出し元を待つとデッドロックするため、`device-bridge-contracts.md`「11」）。
// 全体を `@autoreleasepool` で囲む。
//
// 期限切れの扱い: `cancelWriting` / `cancelExport` を呼び、完了ハンドラを猶予（500 ms。Windows の
// `CANCEL_GRACE` と同値）だけ待つ。
//
// NSError の文面はログにも戻り値にも出さない（security.md A09）。権限・entitlement は不要。
//
// `exportAsynchronouslyWithCompletionHandler:` は macOS 15 / iOS 18 で非推奨だが、deploymentTarget
// （macOS 11・iOS 17）では使える。後継の `export(to:as:isolation:)` は macOS 15 以降のみのため使わず、
// `#pragma clang diagnostic` で非推奨警告だけを抑える。

#import <AVFoundation/AVFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>

#include <os/lock.h>
#include <time.h>
#include <unistd.h>

typedef bool (*fandhe_fill_frame_fn)(void *ctx, uint32_t index, uint8_t *base, size_t bytes_per_row,
                                     size_t len);

// 取り消しても完了ハンドラが猶予内に届かなかった操作の semaphore（6 を返した呼び出しが置く）。
static os_unfair_lock g_unsettled_lock = OS_UNFAIR_LOCK_INIT;
static dispatch_semaphore_t g_unsettled = nil;

// 取り消し後に完了ハンドラを待つ猶予（ns）。
static const uint64_t kCancelGraceNs = 500ull * 1000 * 1000;

static uint64_t fandhe_now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }

// 期限（絶対 ns）までの残り。過ぎていれば 0。
static uint64_t fandhe_remaining_ns(uint64_t deadline) {
    uint64_t now = fandhe_now_ns();
    return now >= deadline ? 0 : deadline - now;
}

// semaphore を `ns` だけ待つ。届けば真。
static bool fandhe_wait(dispatch_semaphore_t sem, uint64_t ns) {
    if (ns > (uint64_t)INT64_MAX) {
        ns = (uint64_t)INT64_MAX;
    }
    return dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, (int64_t)ns)) == 0;
}

// 取り消した操作の完了ハンドラを猶予だけ待つ。届けば 5、届かなければ semaphore を預けて 6。
static int32_t fandhe_settle_or_park(dispatch_semaphore_t sem) {
    if (fandhe_wait(sem, kCancelGraceNs)) {
        return 5;
    }
    os_unfair_lock_lock(&g_unsettled_lock);
    g_unsettled = sem;
    os_unfair_lock_unlock(&g_unsettled_lock);
    return 6;
}

bool fandhe_apple_video_transcode_settled(void) {
    os_unfair_lock_lock(&g_unsettled_lock);
    bool settled = true;
    if (g_unsettled != nil) {
        if (dispatch_semaphore_wait(g_unsettled, DISPATCH_TIME_NOW) == 0) {
            g_unsettled = nil;
        } else {
            settled = false;
        }
    }
    os_unfair_lock_unlock(&g_unsettled_lock);
    return settled;
}

// 1 段目: 元動画（H.264 の MP4）を 1 枚ずつ pool から取って書く。
static int32_t fandhe_write_source(NSURL *url, uint32_t width, uint32_t height, uint32_t fps,
                                   uint32_t frames, uint64_t deadline, void *ctx,
                                   fandhe_fill_frame_fn fill_frame) {
    @autoreleasepool {
        NSError *error = nil;
        AVAssetWriter *writer = [AVAssetWriter assetWriterWithURL:url
                                                         fileType:AVFileTypeMPEG4
                                                            error:&error];
        if (writer == nil) {
            return 2;
        }
        NSDictionary<NSString *, id> *settings = @{
            AVVideoCodecKey : AVVideoCodecTypeH264,
            AVVideoWidthKey : @(width),
            AVVideoHeightKey : @(height),
        };
        if (![writer canApplyOutputSettings:settings forMediaType:AVMediaTypeVideo]) {
            return 4;
        }
        AVAssetWriterInput *input = [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeVideo
                                                                       outputSettings:settings];
        input.expectsMediaDataInRealTime = NO;
        NSDictionary<NSString *, id> *attrs = @{
            (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA),
            (id)kCVPixelBufferWidthKey : @(width),
            (id)kCVPixelBufferHeightKey : @(height),
        };
        AVAssetWriterInputPixelBufferAdaptor *adaptor = [AVAssetWriterInputPixelBufferAdaptor
            assetWriterInputPixelBufferAdaptorWithAssetWriterInput:input
                                       sourcePixelBufferAttributes:attrs];
        if (![writer canAddInput:input]) {
            return 2;
        }
        [writer addInput:input];
        if (![writer startWriting]) {
            return 2;
        }
        [writer startSessionAtSourceTime:kCMTimeZero];
        if (adaptor.pixelBufferPool == NULL) {
            [writer cancelWriting];
            return 2;
        }
        for (uint32_t i = 0; i < frames; i++) {
            // 準備ができるまで締切まで短い間隔で待つ。
            while (!input.readyForMoreMediaData) {
                if (writer.status != AVAssetWriterStatusWriting) {
                    [writer cancelWriting];
                    return 2;
                }
                if (fandhe_remaining_ns(deadline) == 0) {
                    [writer cancelWriting];
                    return 5;
                }
                usleep(2000);
            }
            if (fandhe_remaining_ns(deadline) == 0) {
                [writer cancelWriting];
                return 5;
            }
            CVPixelBufferRef pixel = NULL;
            if (CVPixelBufferPoolCreatePixelBuffer(NULL, adaptor.pixelBufferPool, &pixel) !=
                    kCVReturnSuccess ||
                pixel == NULL) {
                [writer cancelWriting];
                return 2;
            }
            bool drawn = false;
            if (CVPixelBufferLockBaseAddress(pixel, 0) == kCVReturnSuccess) {
                uint8_t *base = (uint8_t *)CVPixelBufferGetBaseAddress(pixel);
                size_t bytes_per_row = CVPixelBufferGetBytesPerRow(pixel);
                size_t len = bytes_per_row * CVPixelBufferGetHeight(pixel);
                drawn = base != NULL && fill_frame(ctx, i, base, bytes_per_row, len);
                CVPixelBufferUnlockBaseAddress(pixel, 0);
            }
            BOOL appended =
                drawn && [adaptor appendPixelBuffer:pixel
                               withPresentationTime:CMTimeMake((int64_t)i, (int32_t)fps)];
            CVPixelBufferRelease(pixel);
            if (!appended) {
                [writer cancelWriting];
                return 2;
            }
        }
        [input markAsFinished];
        [writer endSessionAtSourceTime:CMTimeMake((int64_t)frames, (int32_t)fps)];
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        // 完了ハンドラが writer を保持し、完了まで解放しない（6 のときの安全のため）。
        [writer finishWritingWithCompletionHandler:^{
            (void)writer;
            dispatch_semaphore_signal(done);
        }];
        if (!fandhe_wait(done, fandhe_remaining_ns(deadline))) {
            // finishWriting 開始後の cancelWriting は例外になるため呼ばず、完了を猶予だけ待つ。
            return fandhe_settle_or_park(done);
        }
        return writer.status == AVAssetWriterStatusCompleted ? 0 : 2;
    }
}

// 2 段目: 書いた元動画を `AVAssetExportSession`（640x480 プリセット）で小さい MP4 へ書き出す。
static int32_t fandhe_export(NSURL *src, NSURL *out, uint64_t deadline) {
    @autoreleasepool {
        AVURLAsset *asset = [AVURLAsset URLAssetWithURL:src options:nil];
        AVAssetExportSession *session =
            [[AVAssetExportSession alloc] initWithAsset:asset
                                             presetName:AVAssetExportPreset640x480];
        if (session == nil) {
            return 4;
        }
        session.outputURL = out;
        session.outputFileType = AVFileTypeMPEG4;
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        // 完了ハンドラが session を保持し、完了まで解放しない（6 のときの安全のため）。
        [session exportAsynchronouslyWithCompletionHandler:^{
            (void)session;
            dispatch_semaphore_signal(done);
        }];
#pragma clang diagnostic pop
        if (!fandhe_wait(done, fandhe_remaining_ns(deadline))) {
            [session cancelExport];
            return fandhe_settle_or_park(done);
        }
        return session.status == AVAssetExportSessionStatusCompleted ? 0 : 3;
    }
}

int32_t fandhe_apple_video_transcode_run(const char *src_path, const char *out_path, uint32_t width,
                                         uint32_t height, uint32_t fps, uint32_t frames,
                                         uint64_t remaining_ns, void *ctx,
                                         fandhe_fill_frame_fn fill_frame) {
    @autoreleasepool {
        if (src_path == NULL || out_path == NULL || fill_frame == NULL || width == 0 ||
            height == 0 || fps == 0 || frames == 0 || fps > INT32_MAX || frames > INT32_MAX) {
            return 1;
        }
        NSString *src_string = [NSString stringWithUTF8String:src_path];
        NSString *out_string = [NSString stringWithUTF8String:out_path];
        if (src_string == nil || out_string == nil) {
            return 1;
        }
        uint64_t now = fandhe_now_ns();
        uint64_t deadline = remaining_ns > UINT64_MAX - now ? UINT64_MAX : now + remaining_ns;
        NSURL *src = [NSURL fileURLWithPath:src_string];
        NSURL *out = [NSURL fileURLWithPath:out_string];
        int32_t code = fandhe_write_source(src, width, height, fps, frames, deadline, ctx, fill_frame);
        if (code != 0) {
            return code;
        }
        if (fandhe_remaining_ns(deadline) == 0) {
            return 5;
        }
        return fandhe_export(src, out, deadline);
    }
}
