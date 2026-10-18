// Phase 11 の拡充ループ（親 #100）: OS 管理のバックグラウンドダウンロードを Apple（macOS / iOS）へ
// 橋渡しする（Issue #390 macOS、iOS は b-ios #391）。background `NSURLSession` は iOS と macOS で
// 同一 API のため 1 ファイルを共用する（Foundation のみ、UIKit / AppKit 非依存）。
//
// `crates/fandhe-egui-proxy/src/device/apple_background_download.rs` から C ABI 経由で呼ばれ、
// delegate の通知は同ファイルの `fandhe_rust_bgdl_*`（`#[no_mangle]`）へ返す。`build.rs` が `cc` で
// コンパイルする。コード → 契約上の結果の写像は Rust 側が SSOT で、ここでは写さない。
//
// 設計判断:
// - session は identifier 固定で 1 回だけ作る（`dispatch_once`）。再起動後も同じ identifier で
//   OS が転送中の task を返すので、`getAllTasks` で走行中のものを Rust へ再接続させる。
// - delegate queue は直列（`maxConcurrentOperationCount = 1`）。通知は順に Rust へ届く（契約表 11）。
//   Rust 側のロックを持ったままこちらを呼ばない契約なので、delegate から Rust を呼んでも詰まらない。
// - 世代は Rust が採番して `taskDescription` に入れる。各 delegate は task から世代を読み戻して
//   Rust へ渡し、照合は Rust が行う（開始から戻る前の通知も取りこぼさない）。
// - 保存先は NSCachesDirectory/<bundle id>/background-download の固定名 1 ファイル。素の実行体は
//   sandbox が無く ~/Library/Caches を共用するのでサブディレクトリで分ける。サーバー由来の名前は使わない。
//   パスはログにも画面にも出さない。
// - shutdown では session を invalidate しない（転送を OS に続けさせる）。プロセス終了で片付く資源のみ。
// - 各 C 関数は `@autoreleasepool` で囲む（契約表 5）。main queue への `dispatch_sync` はしない。
//
// 契約:
//   int32_t fandhe_apple_bgdl_init(void);            // 0 = 成功 / 1 = session を作れない
//   int32_t fandhe_apple_bgdl_start(const char *url, uint64_t gen); // 0 / 1 = session なし / 2 = URL 不正
//   int32_t fandhe_apple_bgdl_cancel(void);          // 常に 0（冪等）
//   delegate → Rust の終了コード: 0 = 完了 / 1 = 取り消し / 2 = 接続系 / 3 = 保存失敗 / 4 = その他
#import <Foundation/Foundation.h>
#import <stdbool.h>

extern void fandhe_rust_bgdl_progress(uint64_t gen, uint64_t received, int64_t total);
extern bool fandhe_rust_bgdl_accept(uint64_t gen, int32_t http_status, uint64_t bytes, const char *final_url);
extern bool fandhe_rust_bgdl_finished(uint64_t gen, int32_t code, uint64_t bytes);
extern void fandhe_rust_bgdl_reconnected(uint64_t gen);

static NSString *const kSessionId = @"com.fandhe.egui-proxy.background-download";
// `fandhe_device::background_download::FILE_NAME` と一致させる。
static NSString *const kFileName = @"background-download";

static NSURLSession *g_session = nil;

static uint64_t task_gen(NSURLSessionTask *task) {
    return (uint64_t)[task.taskDescription longLongValue];
}

// 保存先のファイル URL。ディレクトリは作っておく。取得できなければ nil。
static NSURL *dest_url(void) {
    NSURL *caches = [[[NSFileManager defaultManager] URLsForDirectory:NSCachesDirectory
                                                             inDomains:NSUserDomainMask] firstObject];
    if (caches == nil) {
        return nil;
    }
    NSString *sub = [[NSBundle mainBundle] bundleIdentifier] ?: @"fandhe-egui-proxy";
    NSURL *dir = [caches URLByAppendingPathComponent:sub isDirectory:YES];
    [[NSFileManager defaultManager] createDirectoryAtURL:dir
                             withIntermediateDirectories:YES
                                              attributes:nil
                                                   error:nil];
    return [dir URLByAppendingPathComponent:kFileName];
}

static void remove_dest(void) {
    NSURL *dest = dest_url();
    if (dest != nil) {
        [[NSFileManager defaultManager] removeItemAtURL:dest error:nil];
    }
}

@interface FandheBgdlDelegate : NSObject <NSURLSessionDownloadDelegate>
@end

@implementation FandheBgdlDelegate

- (void)URLSession:(NSURLSession *)session
                 downloadTask:(NSURLSessionDownloadTask *)task
                 didWriteData:(int64_t)bytesWritten
            totalBytesWritten:(int64_t)totalBytesWritten
    totalBytesExpectedToWrite:(int64_t)totalBytesExpectedToWrite {
    @autoreleasepool {
        int64_t total = totalBytesExpectedToWrite > 0 ? totalBytesExpectedToWrite : -1;
        fandhe_rust_bgdl_progress(task_gen(task), (uint64_t)totalBytesWritten, total);
    }
}

// 一時ファイルは delegate から戻ると消えるので、受け入れるなら同期で保存先へ移す。
- (void)URLSession:(NSURLSession *)session
                 downloadTask:(NSURLSessionDownloadTask *)task
    didFinishDownloadingToURL:(NSURL *)location {
    @autoreleasepool {
        uint64_t gen = task_gen(task);
        NSHTTPURLResponse *http = [task.response isKindOfClass:[NSHTTPURLResponse class]]
                                      ? (NSHTTPURLResponse *)task.response
                                      : nil;
        int32_t status = http != nil ? (int32_t)http.statusCode : -1;
        NSDictionary *attrs = [[NSFileManager defaultManager] attributesOfItemAtPath:location.path error:nil];
        uint64_t bytes = (uint64_t)[attrs fileSize];
        NSString *finalUrl = task.currentRequest.URL.absoluteString ?: @"";
        if (!fandhe_rust_bgdl_accept(gen, status, bytes, finalUrl.UTF8String)) {
            return;  // 拒否（4xx・0 バイト・許可外のリダイレクト先・古い世代）。一時ファイルは OS が捨てる。
        }
        NSURL *dest = dest_url();
        BOOL moved = NO;
        if (dest != nil) {
            [[NSFileManager defaultManager] removeItemAtURL:dest error:nil];
            moved = [[NSFileManager defaultManager] moveItemAtURL:location toURL:dest error:nil];
        }
        bool adopted = fandhe_rust_bgdl_finished(gen, moved ? 0 : 3, moved ? bytes : 0);
        if (moved && !adopted) {
            // 移す間に取り消し・置き換えされた。嘘の成功を残さないよう保存先を消す。
            remove_dest();
        }
    }
}

// 失敗・取り消しの通知。成功（error == nil）は上の didFinishDownloadingToURL で報告済み。
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task didCompleteWithError:(NSError *)error {
    @autoreleasepool {
        if (error == nil) {
            return;
        }
        int32_t code = 4;
        if ([error.domain isEqualToString:NSURLErrorDomain]) {
            code = error.code == NSURLErrorCancelled ? 1 : 2;
        }
        fandhe_rust_bgdl_finished(task_gen(task), code, 0);
    }
}

@end

int32_t fandhe_apple_bgdl_init(void) {
    @autoreleasepool {
        static dispatch_once_t once;
        dispatch_once(&once, ^{
            NSURLSessionConfiguration *config =
                [NSURLSessionConfiguration backgroundSessionConfigurationWithIdentifier:kSessionId];
            if (config == nil) {
                return;
            }
            NSOperationQueue *queue = [[NSOperationQueue alloc] init];
            queue.maxConcurrentOperationCount = 1;
            g_session = [NSURLSession sessionWithConfiguration:config
                                                      delegate:[[FandheBgdlDelegate alloc] init]
                                                 delegateQueue:queue];
            // 再起動前の転送が OS 側で続いていれば Rust へ再接続させる。
            [g_session getAllTasksWithCompletionHandler:^(NSArray<__kindof NSURLSessionTask *> *tasks) {
                for (NSURLSessionTask *task in tasks) {
                    if (task.state == NSURLSessionTaskStateRunning && task_gen(task) > 0) {
                        fandhe_rust_bgdl_reconnected(task_gen(task));
                    }
                }
            }];
        });
        return g_session != nil ? 0 : 1;
    }
}

int32_t fandhe_apple_bgdl_start(const char *url, uint64_t gen) {
    @autoreleasepool {
        if (g_session == nil) {
            return 1;
        }
        NSString *str = url != NULL ? [NSString stringWithUTF8String:url] : nil;
        NSURL *nsurl = str != nil ? [NSURL URLWithString:str] : nil;
        if (nsurl == nil) {
            return 2;
        }
        // 置き換え: 前の転送を止める。新しい task（世代 gen）は残す。
        [g_session getAllTasksWithCompletionHandler:^(NSArray<__kindof NSURLSessionTask *> *tasks) {
            for (NSURLSessionTask *task in tasks) {
                if (task_gen(task) != gen) {
                    [task cancel];
                }
            }
        }];
        remove_dest();
        NSURLSessionDownloadTask *task = [g_session downloadTaskWithURL:nsurl];
        task.taskDescription = [NSString stringWithFormat:@"%llu", (unsigned long long)gen];
        [task resume];
        return 0;
    }
}

int32_t fandhe_apple_bgdl_cancel(void) {
    @autoreleasepool {
        if (g_session != nil) {
            [g_session getAllTasksWithCompletionHandler:^(NSArray<__kindof NSURLSessionTask *> *tasks) {
                for (NSURLSessionTask *task in tasks) {
                    [task cancel];
                }
            }];
        }
        remove_dest();
        return 0;
    }
}
