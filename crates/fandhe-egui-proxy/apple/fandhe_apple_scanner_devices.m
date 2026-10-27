// Phase 11 の拡充ループ（親 #192）型サンプル「接続したスキャナー・カメラ機器からの取り込み」:
// macOS 実装（Issue #981）。iOS 実装（#982）と共用できるよう、ImageCaptureCore / ImageIO /
// Foundation だけを使い UIKit / AppKit に触れない。iOS の許可（`requestContentsAuthorization`）は
// #982 が `#if TARGET_OS_IOS` の区画へ足す。
//
// `crates/fandhe-egui-proxy/src/device/apple_scanner_devices.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS・iOS ターゲットの feature `device` 有効時に `cc` コンパイルする
// （Rust 側 cfg と同じ OS 集合に保つ）。写像の SSOT は Rust 側の純関数 `failure_for` / `failure_for_os_code`。
//
// 契約（ブロッキング型。Rust のワーカースレッドから呼ぶ。main スレッドから呼ぶと 8）:
//   fandhe_apple_scanner_devices_import(deadline_ms, discovery_ms, max_bytes, ctx, on_bytes,
//                                       out_kind, out_found, out_os_code) -> int32_t
//     deadline_ms : 呼び出し全体の期限（残り時間）。切れたらセッション・探索・ダウンロードを
//                   解除し終えてから 6 を返す
//     discovery_ms: 機器を探す時間（探索窓）
//     max_bytes   : 返す JPEG のバイト数上限（超えたら 10。Rust の領域へは渡さない）
//     on_bytes    : 成功のときだけ、この関数が戻る直前に呼び出しスレッド自身が 1 回だけ呼ぶ。
//                   `bytes` は呼び出しの間だけ有効。期限切れ・失敗の後に Rust の領域へは書かない
//     out_kind    : 0 = カメラ機器
//     out_found   : 探索窓の間に見つかった機器の台数（名前・シリアルは読まない）
//     out_os_code : 9 のときだけ `NSError.code`（0 は欠けた要素など OS のコードが無い失敗）
//     0 成功 / 1 非対応 / 2 機器なし / 3 画像なし / 4 ロック中 / 5 権限拒否 / 6 期限切れ /
//     7 中断（機器が外れた） / 8 main スレッドから呼ばれた / 9 OS の失敗 / 10 上限超過 /
//     11 画像を復号できない
//
// スレッド: ImageCaptureCore の操作（ブラウザの開始・セッションの開閉）は main queue で行い、
// 呼び出しスレッドは semaphore で待つ。完了は `finish` の短いロック区間で 1 回だけ確定し、
// その内側で dispatch しない。後片付け（ブラウザ停止・セッション閉鎖・ダウンロード取消）は期限切れを
// 含む全経路で、呼び出しスレッドが main への `dispatch_sync` で済ませてから戻る（main 側は
// 呼び出し元を待たない）。一時ディレクトリは要求ごとに UUID 名で作り、全経路で削除する。後片付けの
// 後に届いたダウンロード完了はファイルを消すだけにする。
//
// 機器名・シリアル・画像の中身・NSError の文言はログへ出さない（security.md A09）。

#import <Foundation/Foundation.h>
#import <ImageCaptureCore/ImageCaptureCore.h>
#import <ImageIO/ImageIO.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

// ダウンロードする元ファイルの上限（縮小前）。超えたら上限超過として扱う。
static const unsigned long long kFandheMaxSourceBytes = 64ull * 1024ull * 1024ull;
// 縮小後の長辺（画素）。
static const int kFandheMaxPixel = 2048;

@interface FandheScanState : NSObject <ICDeviceBrowserDelegate, ICCameraDeviceDelegate>
@property(nonatomic, strong) dispatch_semaphore_t sem;
// セッション閉鎖の完了を呼び出しスレッドへ伝える（`signalClosed` が 1 回だけ起こす）。
@property(nonatomic, strong) dispatch_semaphore_t closeSem;
@property(nonatomic, strong) NSURL *tempDir;
@property(nonatomic) uint64_t maxBytes;
// 呼び出し全体の期限（`clock_gettime_nsec_np(CLOCK_UPTIME_RAW)` 基準の単調時刻）。復号後に再確認する。
@property(nonatomic) uint64_t deadlineNs;
@property(nonatomic, strong, nullable) ICDeviceBrowser *browser;
@property(nonatomic, strong, nullable) ICCameraDevice *device;
// 任意のキューから読み書きされるため atomic にする。
@property(atomic) BOOL downloading;
@property(nonatomic) BOOL discoveryEnded;
@property(atomic) BOOL picked;
@property(nonatomic) uint32_t found;
// 以下は `lock` の内側でだけ読み書きする。
@property(nonatomic) BOOL done;
// 開始要求が未完了（completion 待ち）。teardown が閉鎖を開始完了まで遅らせる根拠。
@property(nonatomic) BOOL opening;
@property(nonatomic) BOOL tornDown;
@property(nonatomic) BOOL closeRequested;
@property(nonatomic) BOOL closeSignaled;
@property(nonatomic) BOOL closeWait;
// セッションが閉じた（または機器が外れた）ことを OS から確認できた。`lock` の内側でだけ読み書きする。
@property(nonatomic) BOOL closeConfirmed;
@property(nonatomic) int32_t code;
@property(nonatomic) int64_t osCode;
@property(nonatomic, strong, nullable) NSData *jpeg;
@end

@implementation FandheScanState {
    NSObject *_lock;
    NSMutableArray<ICCameraDevice *> *_cameras;
}

- (instancetype)init {
    if ((self = [super init])) {
        _lock = [NSObject new];
        _sem = dispatch_semaphore_create(0);
        _closeSem = dispatch_semaphore_create(0);
        _cameras = [NSMutableArray array];
    }
    return self;
}

// 結果を 1 回だけ確定して待機側を起こす。2 回目以降は無視する。
- (void)finish:(int32_t)code os:(int64_t)os jpeg:(NSData *)jpeg {
    BOOL first = NO;
    @synchronized(_lock) {
        if (!self.done) {
            self.done = YES;
            self.code = code;
            self.osCode = os;
            self.jpeg = jpeg;
            first = YES;
        }
    }
    if (first) {
        dispatch_semaphore_signal(self.sem);
    }
}

- (BOOL)isDone {
    @synchronized(_lock) {
        return self.done;
    }
}

- (void)snapshotCode:(int32_t *)code os:(int64_t *)os jpeg:(NSData **)jpeg {
    @synchronized(_lock) {
        *code = self.code;
        *os = self.osCode;
        *jpeg = self.jpeg;
    }
}

// OS のエラーを戻りコード 9 と NSError.code で確定する（コードを潰さない）。
- (void)failWithError:(NSError *)error {
    [self finish:9 os:(int64_t)(error != nil ? error.code : 0) jpeg:nil];
}

// main queue: ブラウザを開始し、探索窓の終わりを予約する。後片付けが先に走っていたら何もしない。
- (void)startWithDiscoveryMs:(uint64_t)discoveryMs {
    if ([self isDone]) {
        return;
    }
    ICDeviceBrowser *browser = [[ICDeviceBrowser alloc] init];
    browser.delegate = self;
    browser.browsedDeviceTypeMask =
        (ICDeviceTypeMask)(ICDeviceTypeMaskCamera | ICDeviceLocationTypeMaskLocal);
    self.browser = browser;
    [browser start];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(discoveryMs * NSEC_PER_MSEC)),
                   dispatch_get_main_queue(), ^{
                       [self endDiscovery];
                   });
}

// main queue: 探索を打ち切り、0 台なら 2、1 台以上なら先頭の 1 台のセッションを開く。
- (void)endDiscovery {
    if (self.discoveryEnded || [self isDone]) {
        return;
    }
    self.discoveryEnded = YES;
    self.found = (uint32_t)_cameras.count;
    if (_cameras.count == 0) {
        [self finish:2 os:0 jpeg:nil];
        return;
    }
    ICCameraDevice *device = _cameras.firstObject;
    self.device = device;
    if (device.isAccessRestrictedAppleDevice) {
        [self finish:4 os:0 jpeg:nil];
        return;
    }
    device.delegate = self;
    @synchronized(_lock) {
        self.opening = YES;
    }
    [device requestOpenSessionWithOptions:nil
                               completion:^(NSError *_Nullable error) {
                                   [self openDone:error];
                               }];
}

// 開始 completion。後片付けの後に成功で返ったセッションも、その場で閉じる。
- (void)openDone:(NSError *)error {
    BOOL close = NO;
    BOOL signal = NO;
    @synchronized(_lock) {
        self.opening = NO;
        if (self.tornDown && self.closeWait) {
            if (error == nil && !self.closeRequested) {
                self.closeRequested = YES;
                close = YES;
            } else if (error != nil) {
                signal = YES;
            }
        }
    }
    if (close) {
        // ImageCaptureCore の開閉は main queue で行う（completion の queue からは直接呼ばない）。
        dispatch_async(dispatch_get_main_queue(), ^{
            [self.device requestCloseSession];
        });
    } else if (signal) {
        [self signalClosed:YES];
    } else if (error != nil) {
        [self failWithError:error];
    }
}

// 閉鎖待ちを打ち切った状態（OS の後片付けが未確認）。確認できるまで次の要求を受け付けない。
static FandheScanState *gFandheUnclosed = nil;
static NSObject *gFandheUnclosedLock = nil;

// 閉鎖待ちを起こす。`confirmed` が真のときだけ閉鎖確認として未閉鎖の記録を外し delegate を手放す。
- (void)signalClosed:(BOOL)confirmed {
    BOOL wake = NO;
    @synchronized(_lock) {
        if (!self.closeSignaled) {
            self.closeSignaled = YES;
            wake = YES;
        }
        if (confirmed) {
            self.closeConfirmed = YES;
        }
    }
    if (wake) {
        dispatch_semaphore_signal(self.closeSem);
    }
    if (confirmed) {
        @synchronized(gFandheUnclosedLock) {
            if (gFandheUnclosed == self) {
                gFandheUnclosed = nil;
                dispatch_async(dispatch_get_main_queue(), ^{
                    self.device.delegate = nil;
                });
            }
        }
    }
}

// main queue: 未閉鎖の状態を OS の観測で再確認する。閉じていれば確認扱いにし、開いたままなら閉鎖を再要求する。
- (BOOL)recheckClosed {
    BOOL stillOpen = NO;
    @synchronized(_lock) {
        stillOpen = self.opening || self.device.hasOpenSession;
    }
    if (stillOpen) {
        if (self.device.hasOpenSession) {
            [self.device requestCloseSession];
        }
        return NO;
    }
    [self signalClosed:YES];
    return YES;
}

// 閉鎖を確認できていない（teardown の後に呼ぶ）。
- (BOOL)closeUnconfirmed {
    @synchronized(_lock) {
        return self.closeWait && !self.closeConfirmed;
    }
}

// セッションが開いて内容一覧が揃った後に、最新の画像 1 枚を選んでダウンロードする。
- (void)pickAndDownload {
    if (self.picked || [self isDone]) {
        return;
    }
    self.picked = YES;
    ICCameraDevice *device = self.device;
    NSArray<ICCameraItem *> *items = device.mediaFiles;
    if (items == nil) {
        // 一覧が取れない（OS の値が欠けている）。0 件の成功へは畳まない。
        [self finish:9 os:0 jpeg:nil];
        return;
    }
    ICCameraFile *newest = nil;
    for (ICCameraItem *item in items) {
        if (![item isKindOfClass:[ICCameraFile class]] || item.UTI == nil) {
            [self finish:9 os:0 jpeg:nil];
            return;
        }
        UTType *type = [UTType typeWithIdentifier:item.UTI];
        if (type == nil || ![type conformsToType:UTTypeImage] || item.isRaw) {
            continue;
        }
        // 日時が欠けた画像を飛ばして成功を作らない。
        if (item.creationDate == nil) {
            [self finish:9 os:0 jpeg:nil];
            return;
        }
        if (newest == nil || [item.creationDate compare:newest.creationDate] == NSOrderedDescending) {
            newest = (ICCameraFile *)item;
        }
    }
    if (newest == nil) {
        [self finish:3 os:0 jpeg:nil];
        return;
    }
    if (newest.fileSize > 0 && (unsigned long long)newest.fileSize > kFandheMaxSourceBytes) {
        [self finish:10 os:0 jpeg:nil];
        return;
    }
    NSDictionary *options = @{ICDownloadsDirectoryURL : self.tempDir, ICOverwrite : @YES};
    self.downloading = YES;
    [newest requestDownloadWithOptions:options
                            completion:^(NSString *_Nullable filename, NSError *_Nullable error) {
                                [self downloadDone:filename error:error];
                            }];
}

// 任意のキュー: ダウンロード完了。後片付けの後なら一時ファイルを消すだけ。
- (void)downloadDone:(NSString *)filename error:(NSError *)error {
    self.downloading = NO;
    if ([self isDone]) {
        [[NSFileManager defaultManager] removeItemAtURL:self.tempDir error:nil];
        return;
    }
    if (error != nil || filename == nil) {
        [self failWithError:error];
        return;
    }
    @autoreleasepool {
        NSURL *file = [self.tempDir URLByAppendingPathComponent:filename.lastPathComponent];
        NSDictionary *attrs = [[NSFileManager defaultManager] attributesOfItemAtPath:file.path
                                                                               error:nil];
        unsigned long long size = [attrs[NSFileSize] unsignedLongLongValue];
        if (attrs == nil) {
            [self finish:9 os:0 jpeg:nil];
            return;
        }
        if (size > kFandheMaxSourceBytes) {
            [self finish:10 os:0 jpeg:nil];
            return;
        }
        // 向き補正つきで長辺 2048 画素以下へ縮め、JPEG にする。
        CGImageSourceRef source = CGImageSourceCreateWithURL((__bridge CFURLRef)file, NULL);
        if (source == NULL) {
            [self finish:11 os:0 jpeg:nil];
            return;
        }
        NSDictionary *thumbOptions = @{
            (__bridge NSString *)kCGImageSourceCreateThumbnailFromImageAlways : @YES,
            (__bridge NSString *)kCGImageSourceCreateThumbnailWithTransform : @YES,
            (__bridge NSString *)kCGImageSourceThumbnailMaxPixelSize : @(kFandheMaxPixel),
        };
        CGImageRef image =
            CGImageSourceCreateThumbnailAtIndex(source, 0, (__bridge CFDictionaryRef)thumbOptions);
        CFRelease(source);
        if (image == NULL) {
            [self finish:11 os:0 jpeg:nil];
            return;
        }
        NSMutableData *out = [NSMutableData data];
        CGImageDestinationRef dest = CGImageDestinationCreateWithData(
            (__bridge CFMutableDataRef)out, (__bridge CFStringRef)UTTypeJPEG.identifier, 1, NULL);
        if (dest == NULL) {
            CGImageRelease(image);
            [self finish:11 os:0 jpeg:nil];
            return;
        }
        NSDictionary *props =
            @{(__bridge NSString *)kCGImageDestinationLossyCompressionQuality : @0.85};
        CGImageDestinationAddImage(dest, image, (__bridge CFDictionaryRef)props);
        BOOL ok = CGImageDestinationFinalize(dest);
        CFRelease(dest);
        CGImageRelease(image);
        if (!ok || out.length == 0) {
            [self finish:11 os:0 jpeg:nil];
            return;
        }
        if (out.length > self.maxBytes) {
            [self finish:10 os:0 jpeg:nil];
            return;
        }
        // 復号に時間を取られて期限を過ぎていたら、成功にせず期限切れとして確定する。
        if (clock_gettime_nsec_np(CLOCK_UPTIME_RAW) > self.deadlineNs) {
            [self finish:6 os:0 jpeg:nil];
            return;
        }
        [self finish:0 os:0 jpeg:out];
    }
}

// main queue: 後片付け。期限切れを含む全経路で呼ぶ。
- (void)teardown {
    self.browser.delegate = nil;
    [self.browser stop];
    ICCameraDevice *device = self.device;
    if (device != nil) {
#if TARGET_OS_OSX
        if (self.downloading) {
            [device cancelDownload];
        }
#endif
        // 閉鎖の完了（didCloseSession）を受けるため、delegate は閉鎖が済むまで外さない。
        BOOL close = NO;
        @synchronized(_lock) {
            self.tornDown = YES;
            if (device.hasOpenSession) {
                self.closeWait = YES;
                if (!self.closeRequested) {
                    self.closeRequested = YES;
                    close = YES;
                }
            } else if (self.opening) {
                // 開始が未完了。completion が成功で返ったら `openDone:` が閉じる。
                self.closeWait = YES;
            }
        }
        if (close) {
            [device requestCloseSession];
        }
    }
}

// 閉鎖待ちが必要か（teardown の後に呼ぶ）。
- (BOOL)needsCloseWait {
    @synchronized(_lock) {
        return self.closeWait && !self.closeSignaled;
    }
}

#pragma mark - ICDeviceBrowserDelegate

- (void)deviceBrowser:(ICDeviceBrowser *)browser
         didAddDevice:(ICDevice *)device
           moreComing:(BOOL)moreComing {
    if (!self.discoveryEnded && [device isKindOfClass:[ICCameraDevice class]]) {
        [_cameras addObject:(ICCameraDevice *)device];
    }
}

- (void)deviceBrowser:(ICDeviceBrowser *)browser
      didRemoveDevice:(ICDevice *)device
            moreGoing:(BOOL)moreGoing {
    if (device == self.device) {
        [self finish:7 os:0 jpeg:nil];
        [self signalClosed:YES];
    } else if (!self.discoveryEnded) {
        // 探索窓の間に外れた機器は候補から除く（先頭に選んで OS の失敗になるのを防ぐ）。
        [_cameras removeObject:(ICCameraDevice *)device];
    }
}

#if TARGET_OS_OSX
// ローカル（USB）機器の列挙が済んだ。0 台でも呼ばれるので、探索窓を待たずに打ち切れる。
- (void)deviceBrowserDidEnumerateLocalDevices:(ICDeviceBrowser *)browser {
    [self endDiscovery];
}
#endif

#pragma mark - ICDeviceDelegate / ICCameraDeviceDelegate

- (void)didRemoveDevice:(ICDevice *)device {
    [self finish:7 os:0 jpeg:nil];
    // 外れた機器のセッションは閉鎖の完了が来ない。後片付けの前後どちらで外れても待たせない。
    [self signalClosed:YES];
}

- (void)device:(ICDevice *)device didOpenSessionWithError:(NSError *)error {
    if (error != nil) {
        [self failWithError:error];
    }
}

- (void)device:(ICDevice *)device didCloseSessionWithError:(NSError *)error {
    if (self.tornDown) {
        // 閉鎖がエラーで返ったときは待機だけ起こし、閉鎖確認としては扱わない（状態と delegate を保持する）。
        [self signalClosed:error == nil];
        return;
    }
    // 取り込みの途中でセッションが閉じた（確定済みなら無視される）。
    if (!self.picked || self.downloading) {
        [self finish:7 os:0 jpeg:nil];
    }
}

- (void)deviceDidBecomeReadyWithCompleteContentCatalog:(ICCameraDevice *)device {
    [self pickAndDownload];
}

- (void)cameraDeviceDidEnableAccessRestriction:(ICDevice *)device {
    [self finish:4 os:0 jpeg:nil];
}

- (void)cameraDeviceDidRemoveAccessRestriction:(ICDevice *)device {
}

- (void)cameraDevice:(ICCameraDevice *)camera didAddItems:(NSArray<ICCameraItem *> *)items {
}

- (void)cameraDevice:(ICCameraDevice *)camera didRemoveItems:(NSArray<ICCameraItem *> *)items {
}

- (void)cameraDevice:(ICCameraDevice *)camera didRenameItems:(NSArray<ICCameraItem *> *)items {
}

- (void)cameraDeviceDidChangeCapability:(ICCameraDevice *)camera {
}

- (void)cameraDevice:(ICCameraDevice *)camera didReceivePTPEvent:(NSData *)eventData {
}

- (void)cameraDevice:(ICCameraDevice *)camera
    didReceiveThumbnail:(CGImageRef)thumbnail
                forItem:(ICCameraItem *)item
                  error:(NSError *)error {
}

- (void)cameraDevice:(ICCameraDevice *)camera
    didReceiveMetadata:(NSDictionary *)metadata
               forItem:(ICCameraItem *)item
                 error:(NSError *)error {
}

@end

int32_t fandhe_apple_scanner_devices_import(uint64_t deadline_ms, uint64_t discovery_ms,
                                            uint64_t max_bytes, void *ctx,
                                            void (*on_bytes)(void *ctx, const uint8_t *bytes,
                                                             size_t len),
                                            int32_t *out_kind, uint32_t *out_found,
                                            int64_t *out_os_code) {
    @autoreleasepool {
        // ブラウザの開始・後片付けは main への同期呼び出しを使うため、main からは呼べない。
        if ([NSThread isMainThread]) {
            return 8;
        }
        if (@available(macOS 11.0, iOS 14.0, *)) {
        } else {
            return 1;
        }
        *out_kind = 0;
        *out_found = 0;
        *out_os_code = 0;

        static dispatch_once_t once;
        dispatch_once(&once, ^{
            gFandheUnclosedLock = [NSObject new];
        });
        // 前回の閉鎖が未確認の間は OS を触らない（開いたままのセッションへ重ねて開かない）。
        FandheScanState *prev = nil;
        @synchronized(gFandheUnclosedLock) {
            prev = gFandheUnclosed;
        }
        if (prev != nil) {
            // 閉じたかを OS の観測で確かめる。開いたままなら閉鎖を再要求して今回は失敗にする。
            __block BOOL closed = NO;
            dispatch_sync(dispatch_get_main_queue(), ^{
                closed = [prev recheckClosed];
            });
            if (!closed) {
                return 9;
            }
        }

        FandheScanState *state = [[FandheScanState alloc] init];
        state.maxBytes = max_bytes;
        state.deadlineNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW) + deadline_ms * NSEC_PER_MSEC;
        state.tempDir = [NSURL
            fileURLWithPath:[NSTemporaryDirectory()
                                stringByAppendingPathComponent:
                                    [@"fandhe-scan-" stringByAppendingString:[NSUUID UUID]
                                                                                  .UUIDString]]
                isDirectory:YES];
        NSError *dirError = nil;
        if (![[NSFileManager defaultManager] createDirectoryAtURL:state.tempDir
                                      withIntermediateDirectories:YES
                                                       attributes:@{NSFilePosixPermissions : @0700}
                                                            error:&dirError]) {
            *out_os_code = (int64_t)dirError.code;
            return 9;
        }

        dispatch_async(dispatch_get_main_queue(), ^{
            [state startWithDiscoveryMs:discovery_ms];
        });
        long timedOut = dispatch_semaphore_wait(
            state.sem, dispatch_time(DISPATCH_TIME_NOW, (int64_t)(deadline_ms * NSEC_PER_MSEC)));
        if (timedOut != 0) {
            [state finish:6 os:0 jpeg:nil];
        }
        // 全経路の後片付け。期限切れでも UI キューへ post して戻らず、済ませてから返す。
        dispatch_sync(dispatch_get_main_queue(), ^{
            [state teardown];
        });
        // 閉鎖 completion（開始中だった要求の完了後の閉鎖を含む）を待つ。上限は機器が応答しない場合の保険。
        BOOL closeTimedOut = NO;
        if ([state needsCloseWait]) {
            closeTimedOut = dispatch_semaphore_wait(
                                state.closeSem,
                                dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0;
        }
        // 保持の判定と記録を同じ排他区間で行う。signalClosed が先に確認していれば保持せず、必ず delegate を外す。
        BOOL hold = NO;
        if (closeTimedOut || [state closeUnconfirmed]) {
            @synchronized(gFandheUnclosedLock) {
                hold = [state closeUnconfirmed];
                if (hold) {
                    gFandheUnclosed = state;
                }
            }
        }
        if (!hold) {
            dispatch_sync(dispatch_get_main_queue(), ^{
                state.device.delegate = nil;
            });
        }
        [[NSFileManager defaultManager] removeItemAtURL:state.tempDir error:nil];

        int32_t code = 9;
        int64_t os = 0;
        NSData *jpeg = nil;
        [state snapshotCode:&code os:&os jpeg:&jpeg];
        // 画像を取得済みなら、閉鎖の失敗・待機の期限切れで結果を上書きしない（未閉鎖は gFandheUnclosed が次の要求を止める）。
        *out_found = state.found;
        *out_os_code = os;
        if (code == 0) {
            if (jpeg == nil || jpeg.length > max_bytes) {
                return 10;
            }
            on_bytes(ctx, (const uint8_t *)jpeg.bytes, jpeg.length);
        }
        return code;
    }
}
