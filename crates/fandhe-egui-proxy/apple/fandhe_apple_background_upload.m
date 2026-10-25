// Phase 11 の拡充ループ（親 #170）: OS 管理のバックグラウンド送信を Apple（macOS / iOS）へ橋渡しする
// （Issue #835 macOS、#836 iOS）。background `NSURLSession` は macOS と iOS で同一 API のため 1 ファイルを
// 共用する。iOS だけ背景起動の受け口（`TARGET_OS_IOS` の区画）を持つが、その区画は #836 が足す。
//
// `crates/fandhe-egui-proxy/src/device/apple_background_upload.rs` から C ABI 経由で呼ばれ、delegate の
// 通知は同ファイルの `fandhe_rust_bgul_*`（`#[no_mangle]`）へ返す。`build.rs` が `cc` でコンパイルする。
// コード → 契約上の結果の写像は Rust 側が SSOT で、ここでは写さない。
//
// 設計判断:
// - OS を正にする。送信中か・進み具合は `getAllTasks`（`fandhe_apple_bgul_list`）で毎回読み、ここでは
//   覚えない。delegate は「読み直せ」の合図（`fandhe_rust_bgul_signal`）と、一覧から消える完了の結果
//   （`fandhe_rust_bgul_completed`）を Rust へ返すだけ。
// - session は identifier 固定で 1 回だけ作る（`dispatch_once`）。download の session とは identifier を
//   分ける（背景起動の振り分けを identifier で行うため）。再起動後も同じ identifier で OS が続けた
//   転送に結び付く。shutdown では invalidate しない（OS に転送を続けさせる）。
// - delegate queue は直列（`maxConcurrentOperationCount = 1`）。通知は順に Rust へ届く（契約表 11）。
//   Rust 側のロックを持ったままこちらを呼ばない契約なので、delegate から Rust を呼んでも詰まらない。
// - task の照合は OS が採番する `taskIdentifier`。`resume` の前に `fandhe_rust_bgul_will_resume` で
//   Rust へ渡すので、開始から戻る前に届く完了も照合できる。
// - 送るファイルは NSCachesDirectory/<bundle id>/background-upload の固定名 1 ファイル（atomic に上書き）。
//   背景 session の送信はファイルからだけ（メモリ・stream はアプリ終了後に失敗する、公式）。
//   パスはログにも画面にも出さない。
// - 待ちは単調時計の期限つき（`dispatch_time(DISPATCH_TIME_NOW, ..)`）。main queue への
//   `dispatch_sync` はしない。各 C 関数は `@autoreleasepool` で囲む（契約表 5）。
//
// 契約:
//   int32_t fandhe_apple_bgul_init(void);   // 0 = 成功 / 1 = session を作れない
//   int32_t fandhe_apple_bgul_start(const char *url, const uint8_t *body, size_t len, uint64_t deadline_ms);
//     // 0 / 1 = session なし / 2 = URL 不正 / 3 = ファイルを書けない / 4 = 期限内に前の転送の取り消しを終えられない
//   int32_t fandhe_apple_bgul_cancel(uint64_t deadline_ms);   // 0 / 1 / 4
//   int32_t fandhe_apple_bgul_list(uint64_t deadline_ms, FandheBgulTask *out, size_t cap, size_t *out_len);
//     // 0 / 1 / 4 = 期限切れ。`cap` 件までしか書かない。`out_len` は書いた件数。期限切れのときは `out` に触らない
//   delegate → Rust の終了コード: 0 = エラーなし / 1 = 取り消し / 2 = その他の NSURLErrorDomain / 4 = その他
#import <Foundation/Foundation.h>
#import <stdbool.h>
#import <TargetConditionals.h>

typedef struct {
    uint64_t id;
    int32_t state;  // NSURLSessionTaskState の値（0 = running / 1 = suspended / 2 = canceling / 3 = completed）
    uint64_t sent;
    int64_t expected;  // 総バイト数。不明なら負
} FandheBgulTask;

extern void fandhe_rust_bgul_will_resume(uint64_t task_id);
extern void fandhe_rust_bgul_completed(uint64_t task_id, int32_t code, int32_t http_status, uint64_t sent,
                                       const char *final_url);
extern void fandhe_rust_bgul_signal(void);

static NSString *const kSessionId = @"com.fandhe.egui-proxy.background-upload";
// `fandhe_device::background_upload::FILE_NAME` と一致させる。
static NSString *const kFileName = @"background-upload";

static NSURLSession *g_session = nil;

// 送るファイルの URL。ディレクトリは作っておく。取得できなければ nil。
static NSURL *file_url(void) {
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

// 全 task の `[task cancel]` を呼び終えるまで待つ。期限切れなら true。
// `getAllTasks` の completion は非同期（delegate queue 上）。呼び出しは Rust のワーカースレッドで、
// main queue への待ち合わせは無い。completion は Rust のロックを取らないので、待ちは詰まらず期限内に解ける。
static bool cancel_all_tasks(uint64_t deadline_ms) {
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    [g_session getAllTasksWithCompletionHandler:^(NSArray<__kindof NSURLSessionTask *> *tasks) {
        for (NSURLSessionTask *task in tasks) {
            [task cancel];
        }
        dispatch_semaphore_signal(done);
    }];
    dispatch_time_t limit = dispatch_time(DISPATCH_TIME_NOW, (int64_t)(deadline_ms * NSEC_PER_MSEC));
    return dispatch_semaphore_wait(done, limit) != 0;
}

@interface FandheBgulDelegate : NSObject <NSURLSessionTaskDelegate, NSURLSessionDataDelegate>
@end

@implementation FandheBgulDelegate

// 進み具合が変わった。状態は運ばず、読み直せの合図だけ送る。
- (void)URLSession:(NSURLSession *)session
                        task:(NSURLSessionTask *)task
             didSendBodyData:(int64_t)bytesSent
              totalBytesSent:(int64_t)totalBytesSent
    totalBytesExpectedToSend:(int64_t)totalBytesExpectedToSend {
    @autoreleasepool {
        fandhe_rust_bgul_signal();
    }
}

// 転送の終了（成功・失敗・取り消し）。終わった task は `getAllTasks` から消えるので、結果だけ Rust へ渡す。
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task didCompleteWithError:(NSError *)error {
    @autoreleasepool {
        int32_t code = 0;
        if (error != nil) {
            code = 4;
            if ([error.domain isEqualToString:NSURLErrorDomain]) {
                code = error.code == NSURLErrorCancelled ? 1 : 2;
            }
        }
        NSHTTPURLResponse *http = [task.response isKindOfClass:[NSHTTPURLResponse class]]
                                      ? (NSHTTPURLResponse *)task.response
                                      : nil;
        int32_t status = http != nil ? (int32_t)http.statusCode : -1;
        NSString *finalUrl = task.currentRequest.URL.absoluteString ?: @"";
        fandhe_rust_bgul_completed((uint64_t)task.taskIdentifier, code, status,
                                   (uint64_t)task.countOfBytesSent, finalUrl.UTF8String);
        fandhe_rust_bgul_signal();
    }
}

- (void)URLSession:(NSURLSession *)session didBecomeInvalidWithError:(NSError *)error {
    @autoreleasepool {
        fandhe_rust_bgul_signal();
    }
}

@end

int32_t fandhe_apple_bgul_init(void) {
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
                                                      delegate:[[FandheBgulDelegate alloc] init]
                                                 delegateQueue:queue];
        });
        return g_session != nil ? 0 : 1;
    }
}

int32_t fandhe_apple_bgul_start(const char *url, const uint8_t *body, size_t len, uint64_t deadline_ms) {
    @autoreleasepool {
        if (g_session == nil) {
            return 1;
        }
        NSString *str = url != NULL ? [NSString stringWithUTF8String:url] : nil;
        NSURL *nsurl = str != nil ? [NSURL URLWithString:str] : nil;
        if (nsurl == nil) {
            return 2;
        }
        // 置き換え: 前の転送を、ファイルを書き換える前に止める。依頼が終わらなければ新しい task は作らない。
        if (cancel_all_tasks(deadline_ms)) {
            return 4;
        }
        NSURL *file = file_url();
        if (file == nil || body == NULL ||
            ![[NSData dataWithBytes:body length:len] writeToURL:file options:NSDataWritingAtomic error:nil]) {
            return 3;  // ファイルを用意できなければ task は作らない。
        }
        NSMutableURLRequest *req = [NSMutableURLRequest requestWithURL:nsurl];
        req.HTTPMethod = @"PUT";
        [req setValue:@"application/octet-stream" forHTTPHeaderField:@"Content-Type"];
        NSURLSessionUploadTask *task = [g_session uploadTaskWithRequest:req fromFile:file];
        if (task == nil) {
            return 3;
        }
        // `resume` の前に Rust へ識別子を渡す。開始から戻る前に届く完了も照合できる。
        fandhe_rust_bgul_will_resume((uint64_t)task.taskIdentifier);
        [task resume];
        return 0;
    }
}

int32_t fandhe_apple_bgul_cancel(uint64_t deadline_ms) {
    @autoreleasepool {
        if (g_session == nil) {
            return 1;
        }
        return cancel_all_tasks(deadline_ms) ? 4 : 0;
    }
}

int32_t fandhe_apple_bgul_list(uint64_t deadline_ms, FandheBgulTask *out, size_t cap, size_t *out_len) {
    @autoreleasepool {
        if (out_len != NULL) {
            *out_len = 0;
        }
        if (g_session == nil) {
            return 1;
        }
        // completion は期限後に遅れて走りうるので、`out` ではなくブロックが保持する領域へ書き、
        // 待ちが成功した後でだけ `out` へ写す（呼び出し側のバッファの寿命に依存しない）。
        NSMutableData *buf = [NSMutableData data];
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        [g_session getAllTasksWithCompletionHandler:^(NSArray<__kindof NSURLSessionTask *> *tasks) {
            for (NSURLSessionTask *task in tasks) {
                FandheBgulTask t;
                t.id = (uint64_t)task.taskIdentifier;
                t.state = (int32_t)task.state;
                t.sent = (uint64_t)task.countOfBytesSent;
                t.expected = (int64_t)task.countOfBytesExpectedToSend;
                [buf appendBytes:&t length:sizeof(t)];
            }
            dispatch_semaphore_signal(done);
        }];
        dispatch_time_t limit = dispatch_time(DISPATCH_TIME_NOW, (int64_t)(deadline_ms * NSEC_PER_MSEC));
        if (dispatch_semaphore_wait(done, limit) != 0) {
            return 4;
        }
        size_t n = buf.length / sizeof(FandheBgulTask);
        if (n > cap) {
            n = cap;
        }
        if (n > 0 && out != NULL) {
            memcpy(out, buf.bytes, n * sizeof(FandheBgulTask));
        }
        if (out_len != NULL) {
            *out_len = (out != NULL) ? n : 0;
        }
        return 0;
    }
}
