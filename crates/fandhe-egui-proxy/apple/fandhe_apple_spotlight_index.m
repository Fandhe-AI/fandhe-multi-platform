// Phase 11 の拡充ループ（親 #117、Issue #560 macOS）: OS の検索（Core Spotlight）への登録と、
// 検索結果から開かれたときの受信を Apple（macOS / iOS）へ橋渡しする。
//
// `crates/fandhe-egui-proxy/src/device/apple_spotlight_index.rs` から C ABI 経由で呼ばれる。
// `build.rs` が feature `device` 有効時に `cc` コンパイルし CoreSpotlight をリンクする。
// 登録・削除（CoreSpotlight）は iOS と共用する。受信（`TARGET_OS_OSX` 部）は macOS 専用で、
// iOS の受信は scene 側（`ios/fandhe_ios_scene.m`、#561）が担う。
//
// 契約（登録・削除。任意スレッド＝ワーカーから呼ぶ。文字列は UTF-8 のポインタと長さで、呼び出し中だけ有効）:
//   int32_t fandhe_apple_spotlight_index(id, id_len, title, title_len, text, text_len,
//                                        domain, domain_len, timeout_ms);
//   int32_t fandhe_apple_spotlight_remove_all(domain, domain_len, timeout_ms);
//   戻り値: 0=成功 / -2=引数不正 / -4=非対応（索引が使えない） / -5=失敗（.app でない実行など）
//           / -6=時間切れ / -7=OS が失敗を返した・受け口を足せなかった（macOS の登録）
//   時間切れ後に OS の完了が遅れて届いても、semaphore を signal するだけで無害（block が retain する）。
//   OS のエラー文は返さない（ログにも出さない）。
//
// 契約（受信。macOS のみ。main thread から呼ぶ冪等な関数）:
//   void fandhe_apple_spotlight_register(prefix, prefix_len);
//   `NSApp.delegate` のクラスへ `application:continueUserActivity:restorationHandler:` を足す。
//   winit は `run_native` の内側で delegate を設定するため、delegate が無ければ
//   WillFinishLaunching（delegate 設定後・継続アクティビティの配送前）で足す。既にメソッドが
//   あれば上書きしない。届いた `CSSearchableItemActionType` の識別子（NSString のみ）を
//   prefix + id の URL にして `fandhe_rust_url_scheme_received` へ渡す（検査は Rust の UrlInbox）。
#import <CoreSpotlight/CoreSpotlight.h>
#import <Foundation/Foundation.h>
#import <TargetConditionals.h>

#if TARGET_OS_OSX
#import <AppKit/AppKit.h>
#import <objc/runtime.h>
#import <stdatomic.h>

// 受け口を足せなかった（delegate に同名メソッドが既にあった）とき true。登録しても開かれた識別子を
// 受け取れないので、登録を失敗で返して成功を装わない（契約表 6）。main スレッドが書き、
// 登録のワーカーが読むため atomic にする（main へ dispatch_sync しない。契約表 11）。
static atomic_bool g_install_failed = false;
#endif

static NSString *fandhe_str(const uint8_t *p, size_t len) {
  if (p == NULL || len == 0) {
    return nil;
  }
  return [[NSString alloc] initWithBytes:p length:len encoding:NSUTF8StringEncoding];
}

// 完了ハンドラを timeout_ms まで待って戻り値へ写す。
static int32_t fandhe_wait(dispatch_semaphore_t sem, int64_t timeout_ms) {
  dispatch_time_t t = dispatch_time(DISPATCH_TIME_NOW, timeout_ms * (int64_t)NSEC_PER_MSEC);
  if (dispatch_semaphore_wait(sem, t) != 0) {
    return -6;
  }
  return 0;
}

int32_t fandhe_apple_spotlight_index(const uint8_t *id, size_t id_len, const uint8_t *title,
                                     size_t title_len, const uint8_t *text, size_t text_len,
                                     const uint8_t *domain, size_t domain_len,
                                     int64_t timeout_ms) {
  @autoreleasepool {
    NSString *id_s = fandhe_str(id, id_len);
    NSString *title_s = fandhe_str(title, title_len);
    NSString *domain_s = fandhe_str(domain, domain_len);
    // 本文は空でもよい（NULL・0 は空文字として扱う）。
    NSString *text_s = text_len == 0 ? @"" : fandhe_str(text, text_len);
    if (id_s == nil || title_s == nil || domain_s == nil || text_s == nil || timeout_ms <= 0) {
      return -2;
    }
    if (![CSSearchableIndex isIndexingAvailable]) {
      return -4;
    }
    if ([NSBundle mainBundle].bundleIdentifier == nil) {
      return -5;
    }
#if TARGET_OS_OSX
    if (atomic_load(&g_install_failed)) {
      return -7;
    }
#endif
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    CSSearchableItemAttributeSet *attrs =
        [[CSSearchableItemAttributeSet alloc] initWithItemContentType:@"public.text"];
#pragma clang diagnostic pop
    attrs.title = title_s;
    attrs.contentDescription = text_s;
    CSSearchableItem *item = [[CSSearchableItem alloc] initWithUniqueIdentifier:id_s
                                                               domainIdentifier:domain_s
                                                                   attributeSet:attrs];
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    __block BOOL failed = NO;
    [[CSSearchableIndex defaultSearchableIndex]
        indexSearchableItems:@[ item ]
           completionHandler:^(NSError *error) {
             failed = (error != nil);
             dispatch_semaphore_signal(sem);
           }];
    int32_t waited = fandhe_wait(sem, timeout_ms);
    if (waited != 0) {
      return waited;
    }
    return failed ? -7 : 0;
  }
}

int32_t fandhe_apple_spotlight_remove_all(const uint8_t *domain, size_t domain_len,
                                          int64_t timeout_ms) {
  @autoreleasepool {
    NSString *domain_s = fandhe_str(domain, domain_len);
    if (domain_s == nil || timeout_ms <= 0) {
      return -2;
    }
    if (![CSSearchableIndex isIndexingAvailable]) {
      return -4;
    }
    if ([NSBundle mainBundle].bundleIdentifier == nil) {
      return -5;
    }
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    __block BOOL failed = NO;
    // 自ドメインだけを消す（他のドメイン・他アプリの項目には触れない）。
    [[CSSearchableIndex defaultSearchableIndex]
        deleteSearchableItemsWithDomainIdentifiers:@[ domain_s ]
                                 completionHandler:^(NSError *error) {
                                   failed = (error != nil);
                                   dispatch_semaphore_signal(sem);
                                 }];
    int32_t waited = fandhe_wait(sem, timeout_ms);
    if (waited != 0) {
      return waited;
    }
    return failed ? -7 : 0;
  }
}

#if TARGET_OS_OSX

extern void fandhe_rust_url_scheme_received(const uint8_t *ptr, size_t len);

static NSString *g_prefix = nil;
static BOOL g_installed = NO;
static BOOL g_observing = NO;

static BOOL fandhe_continue_user_activity(id __unused self_, SEL __unused cmd,
                                          NSApplication *__unused app, NSUserActivity *activity,
                                          void (^__unused handler)(NSArray *)) {
  @autoreleasepool {
    if (![activity.activityType isEqualToString:CSSearchableItemActionType]) {
      return NO;
    }
    id ident = activity.userInfo[CSSearchableItemActivityIdentifier];
    if (![ident isKindOfClass:[NSString class]] || g_prefix == nil) {
      return NO;
    }
    NSData *url = [[g_prefix stringByAppendingString:(NSString *)ident]
        dataUsingEncoding:NSUTF8StringEncoding];
    if (url == nil) {
      return NO;
    }
    fandhe_rust_url_scheme_received((const uint8_t *)url.bytes, (size_t)url.length);
    return YES;
  }
}

static void fandhe_try_install(void) {
  if (g_installed || NSApp == nil || NSApp.delegate == nil) {
    return;
  }
  SEL sel = @selector(application:continueUserActivity:restorationHandler:);
  struct objc_method_description d =
      protocol_getMethodDescription(@protocol(NSApplicationDelegate), sel, NO, YES);
  const char *types = d.types != NULL ? d.types : "c@:@@@?";
  // 既にメソッドがあれば上書きしない。足せなければ登録を失敗で返す（g_install_failed）。
  if (!class_addMethod([NSApp.delegate class], sel, (IMP)fandhe_continue_user_activity, types)) {
    atomic_store(&g_install_failed, true);
  }
  g_installed = YES;
}

void fandhe_apple_spotlight_register(const uint8_t *prefix, size_t prefix_len) {
  @autoreleasepool {
    NSString *p = fandhe_str(prefix, prefix_len);
    if (p == nil || ![NSThread isMainThread]) {
      return;
    }
    g_prefix = p;
    fandhe_try_install();
    if (g_observing) {
      return;
    }
    g_observing = YES;
    // winit の delegate は `run_native` の内側で設定されるため、起動通知でも試す。
    for (NSNotificationName name in @[
           NSApplicationWillFinishLaunchingNotification,
           NSApplicationDidFinishLaunchingNotification
         ]) {
      [[NSNotificationCenter defaultCenter] addObserverForName:name
                                                        object:nil
                                                         queue:nil
                                                    usingBlock:^(NSNotification *__unused n) {
                                                      fandhe_try_install();
                                                    }];
    }
  }
}

#endif
