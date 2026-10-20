// Phase 11 の拡充ループ（親 #109）型サンプル「LAN のサービス探索」: macOS（Issue #486）の実装。
// iOS（b-ios）も同じファイルを共用する。Foundation / Network / dns_sd（libSystem）だけを使い、
// AppKit / UIKit は使わない。
//
// `crates/fandhe-egui-proxy/src/device/apple_lan_discovery.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS / macOS の feature `device` 有効時に `cc` でコンパイルし、Network をリンクする。
//
// 発見と解決の分担: `nw_browser`（Bonjour）は endpoint（サービス名）しか返さないため、発見に使い、
// アドレスとポートは dns_sd の `DNSServiceResolve` → `DNSServiceGetAddrInfo` で解決する。
// 探索するサービス種別は `_http._tcp`（`local.`）に固定する。接続・公開（register）はしない。
//
// 契約（pull 型。`fandhe_apple_bluetooth_le.m` と同型）:
//   int32_t fandhe_apple_lan_start(uint64_t request_id, uint64_t duration_ms)
//     常に 0（受理）を返し、処理は専用シリアルキューで非同期に進む。進行中のセッションがあれば
//     完了コールバックなしで破棄して置き換える（古い request_id の結果は Rust 側が照合して捨てる）。
//     duration_ms は Rust 側 `BROWSE_DURATION_MS`（SSOT）で、窓の起点はこの呼び出し（単調時計）。
//   void fandhe_apple_lan_stop(void)
//     終了時専用。専用キューへ同期ディスパッチして browser と dns_sd の参照を解放する。完了
//     コールバックは呼ばない。呼び出し元は Rust 側のロックを握らないこと。
//   fandhe_rust_lan_service(request_id, name, addr_or_null, port)
//   fandhe_rust_lan_finished(request_id, code)
//     Rust 側が公開する `extern "C"` 関数。service は窓が閉じて成功と確定したときだけ、finished の
//     直前にまとめて通知する（未解決は addr = NULL・port = 0）。code: 0 成功 / 1 許可なし /
//     2 ネットワーク未接続 / 3 非対応 / 4 中断・失敗 / 5 種別が未宣言（NoAuth）。意味づけ（契約の
//     エラー種別への写像）は Rust 側 `fandhe_device::lan_discovery::decode_code` に一本化する。
//     1 要求につき finished は高々 1 回（`FandheLanClose` が唯一の発火点）。
//
// スレッド: 専用のシリアルキューだけで動かし、main queue は使わない（main 側が取るロックとの
// デッドロックを避ける、`device-bridge-contracts.md` 11 節）。dns_sd の各 `DNSServiceRef` も
// `DNSServiceSetDispatchQueue` で同じキューに載せ、作成・解放もこのキューで行う（セッション状態は
// すべてこのキューに閉じる）。
//
// 失敗の判定（窓が閉じる時点）: PolicyDenied（-65570）→ 1、NoAuth（-65555）→ 5、browser が
// failed → 4、waiting → 2、0 件で `NWPathMonitor` が接続なし → 2、それ以外は 0（0 件を含む成功）。
// 失敗のときは部分結果を通知しない（嘘の成功を避ける、`device-bridge-contracts.md` 6 節）。
// サービス名・アドレスを NSLog 等へ出力しない（security.md A09）。

#pragma clang diagnostic ignored "-Wunused-parameter" // dns_sd / nw のコールバック署名は固定
#import <Foundation/Foundation.h>
#import <Network/Network.h>
#import <dns_sd.h>
#import <arpa/inet.h>
#import <sys/socket.h>

extern void fandhe_rust_lan_service(uint64_t request_id, const char *name, const char *addr_or_null, uint16_t port);
extern void fandhe_rust_lan_finished(uint64_t request_id, int32_t code);
// `fandhe_apple_network_status.m`。初回の呼び出しで monitor が始まるため start で温める。
extern int32_t fandhe_apple_network_status_read(void);

// 1 回の窓で扱うサービス数の上限（dns_sd の参照数を抑える。Rust 側の返却上限は別に持つ）。
static const NSUInteger kFandheLanMaxItems = 100;
static const DNSServiceErrorType kFandheLanPolicyDenied = -65570; // kDNSServiceErr_PolicyDenied
static const DNSServiceErrorType kFandheLanNoAuth = -65555;       // kDNSServiceErr_NoAuth

@interface FandheLanItem : NSObject
@property(nonatomic, copy) NSString *name;
@property(nonatomic) uint64_t requestId;
@property(nonatomic) DNSServiceRef resolveRef;
@property(nonatomic) DNSServiceRef addrRef;
@property(nonatomic) uint16_t port;
@property(nonatomic, copy) NSString *addrV4;
@property(nonatomic, copy) NSString *addrV6;
@end

@implementation FandheLanItem
@end

@interface FandheLanSession : NSObject
@property(nonatomic) uint64_t requestId;
@property(nonatomic) nw_browser_t browser;
@property(nonatomic, strong) NSMutableDictionary<NSString *, FandheLanItem *> *items;
@property(nonatomic) BOOL denied;
@property(nonatomic) BOOL noAuth;
@property(nonatomic) BOOL waiting;
@property(nonatomic) BOOL failed;
@end

@implementation FandheLanSession
@end

static dispatch_queue_t g_lan_queue;
static FandheLanSession *g_lan_session = nil;

static dispatch_queue_t FandheLanQueue(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      g_lan_queue = dispatch_queue_create("com.fandhe.lan-discovery", DISPATCH_QUEUE_SERIAL);
    });
    return g_lan_queue;
}

// セッションを止めて破棄する（専用キュー上でのみ呼ぶ。コールバックは出さない）。
// dns_sd の参照は browser より先に、すべて同じキュー上で解放する（解放後はコールバックが来ない）。
static void FandheLanTeardown(void) {
    FandheLanSession *session = g_lan_session;
    g_lan_session = nil;
    if (session == nil) {
        return;
    }
    for (FandheLanItem *item in session.items.allValues) {
        if (item.resolveRef != NULL) {
            DNSServiceRefDeallocate(item.resolveRef);
            item.resolveRef = NULL;
        }
        if (item.addrRef != NULL) {
            DNSServiceRefDeallocate(item.addrRef);
            item.addrRef = NULL;
        }
    }
    if (session.browser != nil) {
        nw_browser_cancel(session.browser);
        session.browser = nil; // ハンドラ経由の循環参照を断つ
    }
    [session.items removeAllObjects];
}

// DNS-SD のエラーをセッションの失敗フラグへ記録する（専用キュー上でのみ呼ぶ）。
static void FandheLanRecordError(FandheLanSession *session, DNSServiceErrorType err) {
    if (err == kFandheLanPolicyDenied) {
        session.denied = YES;
    } else if (err == kFandheLanNoAuth) {
        session.noAuth = YES;
    }
}

// 現在のセッションかを確かめて返す（古い・破棄済みなら nil）。
static FandheLanSession *FandheLanCurrent(uint64_t request_id) {
    FandheLanSession *session = g_lan_session;
    return (session != nil && session.requestId == request_id) ? session : nil;
}

static void FandheLanAddrReply(DNSServiceRef sdRef, DNSServiceFlags flags, uint32_t interfaceIndex,
                               DNSServiceErrorType errorCode, const char *hostname,
                               const struct sockaddr *address, uint32_t ttl, void *context) {
    @autoreleasepool {
        FandheLanItem *item = (__bridge FandheLanItem *)context;
        FandheLanSession *session = FandheLanCurrent(item.requestId);
        if (session == nil) {
            return;
        }
        if (errorCode != kDNSServiceErr_NoError) {
            FandheLanRecordError(session, errorCode);
            return;
        }
        if (!(flags & kDNSServiceFlagsAdd) || address == NULL) {
            return;
        }
        char buf[INET6_ADDRSTRLEN];
        if (address->sa_family == AF_INET && item.addrV4 == nil) {
            const struct sockaddr_in *in4 = (const struct sockaddr_in *)address;
            if (inet_ntop(AF_INET, &in4->sin_addr, buf, sizeof(buf)) != NULL) {
                item.addrV4 = [NSString stringWithUTF8String:buf];
            }
        } else if (address->sa_family == AF_INET6 && item.addrV6 == nil) {
            const struct sockaddr_in6 *in6 = (const struct sockaddr_in6 *)address;
            if (inet_ntop(AF_INET6, &in6->sin6_addr, buf, sizeof(buf)) != NULL) {
                item.addrV6 = [NSString stringWithUTF8String:buf];
            }
        }
    }
}

static void FandheLanResolveReply(DNSServiceRef sdRef, DNSServiceFlags flags, uint32_t interfaceIndex,
                                  DNSServiceErrorType errorCode, const char *fullname,
                                  const char *hosttarget, uint16_t port, uint16_t txtLen,
                                  const unsigned char *txtRecord, void *context) {
    @autoreleasepool {
        FandheLanItem *item = (__bridge FandheLanItem *)context;
        FandheLanSession *session = FandheLanCurrent(item.requestId);
        if (session == nil) {
            return;
        }
        if (errorCode != kDNSServiceErr_NoError) {
            FandheLanRecordError(session, errorCode);
            return;
        }
        if (hosttarget == NULL) {
            return;
        }
        item.port = ntohs(port);
        if (item.addrRef != NULL) {
            return; // 解決の更新通知。アドレス問い合わせは 1 回だけ。
        }
        DNSServiceRef ref = NULL;
        DNSServiceErrorType err = DNSServiceGetAddrInfo(
            &ref, 0, interfaceIndex, kDNSServiceProtocol_IPv4 | kDNSServiceProtocol_IPv6, hosttarget,
            FandheLanAddrReply, (__bridge void *)item);
        if (err != kDNSServiceErr_NoError || ref == NULL) {
            FandheLanRecordError(session, err);
            return;
        }
        if (DNSServiceSetDispatchQueue(ref, FandheLanQueue()) != kDNSServiceErr_NoError) {
            DNSServiceRefDeallocate(ref);
            return;
        }
        item.addrRef = ref;
    }
}

// 発見した 1 件の解決を始める（専用キュー上でのみ呼ぶ）。
static void FandheLanAddService(FandheLanSession *session, const char *nameC) {
    if (nameC == NULL || session.items.count >= kFandheLanMaxItems) {
        return;
    }
    NSString *name = [NSString stringWithUTF8String:nameC];
    if (name.length == 0 || session.items[name] != nil) {
        return;
    }
    FandheLanItem *item = [[FandheLanItem alloc] init];
    item.name = name;
    item.requestId = session.requestId;
    session.items[name] = item;

    DNSServiceRef ref = NULL;
    DNSServiceErrorType err = DNSServiceResolve(&ref, 0, kDNSServiceInterfaceIndexAny, nameC, "_http._tcp",
                                                "local.", FandheLanResolveReply, (__bridge void *)item);
    if (err != kDNSServiceErr_NoError || ref == NULL) {
        FandheLanRecordError(session, err);
        return; // 未解決のまま名前だけ残す
    }
    if (DNSServiceSetDispatchQueue(ref, FandheLanQueue()) != kDNSServiceErr_NoError) {
        DNSServiceRefDeallocate(ref);
        return;
    }
    item.resolveRef = ref;
}

// 窓が閉じる時点の終了コードを決める（専用キュー上でのみ呼ぶ）。
static int32_t FandheLanOutcome(FandheLanSession *session) {
    if (session.denied) {
        return 1;
    }
    if (session.noAuth) {
        return 5;
    }
    if (session.failed) {
        return 4;
    }
    if (session.waiting) {
        return 2;
    }
    if (session.items.count == 0 && (fandhe_apple_network_status_read() & 0x3) == 2) {
        return 2;
    }
    return 0;
}

// 完了の唯一の発火点（専用キュー上でのみ呼ぶ）。対象が現在のセッションでなければ何もしない。
// 成功のときだけ、未解決を含む各サービスを通知してから finished を 1 回呼ぶ。
static void FandheLanClose(uint64_t request_id) {
    FandheLanSession *session = FandheLanCurrent(request_id);
    if (session == nil) {
        return;
    }
    int32_t code = FandheLanOutcome(session);
    if (code == 0) {
        for (FandheLanItem *item in session.items.allValues) {
            NSString *addr = item.addrV4 != nil ? item.addrV4 : item.addrV6;
            fandhe_rust_lan_service(request_id, item.name.UTF8String, addr.UTF8String, item.port);
        }
    }
    FandheLanTeardown();
    fandhe_rust_lan_finished(request_id, code);
}

int32_t fandhe_apple_lan_start(uint64_t request_id, uint64_t duration_ms) {
    // 窓の起点はこの呼び出し（dispatch_time は単調時計）。
    dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW, (int64_t)(duration_ms * NSEC_PER_MSEC));
    dispatch_async(FandheLanQueue(), ^{
      @autoreleasepool {
          // 進行中のセッションは置き換える（コールバックなし）。
          FandheLanTeardown();
          (void)fandhe_apple_network_status_read(); // monitor を温める

          FandheLanSession *session = [[FandheLanSession alloc] init];
          session.requestId = request_id;
          session.items = [NSMutableDictionary dictionary];

          nw_browse_descriptor_t descriptor = nw_browse_descriptor_create_bonjour_service("_http._tcp", "local.");
          nw_browser_t browser = nw_browser_create(descriptor, NULL);
          if (browser == nil) {
              fandhe_rust_lan_finished(request_id, 3);
              return;
          }
          nw_browser_set_queue(browser, FandheLanQueue());
          nw_browser_set_browse_results_changed_handler(
              browser, ^(nw_browse_result_t old_result, nw_browse_result_t new_result, bool batch_complete) {
                @autoreleasepool {
                    // 追加（old が無く new がある）のときだけ解決へ回す。
                    if (old_result != nil || new_result == nil || FandheLanCurrent(request_id) != session) {
                        return;
                    }
                    nw_endpoint_t endpoint = nw_browse_result_copy_endpoint(new_result);
                    if (endpoint != nil) {
                        FandheLanAddService(session, nw_endpoint_get_bonjour_service_name(endpoint));
                    }
                }
              });
          nw_browser_set_state_changed_handler(browser, ^(nw_browser_state_t state, nw_error_t error) {
            @autoreleasepool {
                if (FandheLanCurrent(request_id) != session) {
                    return;
                }
                session.waiting = (state == nw_browser_state_waiting);
                if (state == nw_browser_state_failed) {
                    session.failed = YES;
                }
                if (error != nil && nw_error_get_error_domain(error) == nw_error_domain_dns) {
                    FandheLanRecordError(session, (DNSServiceErrorType)nw_error_get_error_code(error));
                }
            }
          });
          session.browser = browser;
          g_lan_session = session;
          nw_browser_start(browser);

          dispatch_after(deadline, FandheLanQueue(), ^{
            @autoreleasepool {
                FandheLanClose(request_id);
            }
          });
      }
    });
    return 0;
}

void fandhe_apple_lan_stop(void) {
    dispatch_sync(FandheLanQueue(), ^{
      @autoreleasepool {
          FandheLanTeardown();
      }
    });
}
