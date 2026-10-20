// Phase 11 の拡充ループ（親 #110）型サンプル「近くの端末との直接通信」: macOS（Issue #511）の実装。
// iOS（Issue #512）も同じファイルを共用する。共通部分は Foundation と MultipeerConnectivity だけを使い、
// UIKit は `#if TARGET_OS_IPHONE` の背景遷移の購読にだけ使う（AppKit は使わない）。
//
// `crates/fandhe-egui-proxy/src/device/apple_local_peer.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS / macOS の feature `device` 有効時に `cc` でコンパイルし、MultipeerConnectivity を
// リンクする。
//
// 動作: 同じサービス種別（"fandhe-p2p"）を advertise と browse の両方で行い、見つけた相手のうち
// 1 台だけと暗号化必須（MCEncryptionRequired）のセッションを張り、固定文字列を 1 回送り、相手の
// 1 回を受け取って閉じる。どちらが招待するかは表示名のバイト順（strcmp）で決める
// （Rust 側 `fandhe_device::local_peer::should_invite` と同じ規則。SSOT は Rust 側）。
// 相手は認証しない（同じ種別を名乗る別インスタンスは区別できない。守らない範囲）。
//
// 契約（pull 型。`fandhe_apple_lan_discovery.m` と同型）:
//   int32_t fandhe_apple_local_peer_start(request_id, own_name, outgoing, outgoing_len, window_ms)
//     引数を同期でコピーしてから 0（受理）を返し、処理は専用シリアルキューで非同期に進む。引数が
//     不正（NULL・上限超過）なら 1 を返して何もしない。進行中のセッションがあれば完了コールバック
//     なしで破棄して置き換える（古い request_id の結果は Rust 側が照合して捨てる）。
//     window_ms は Rust 側 `EXCHANGE_WINDOW_MS`（SSOT）で、窓の起点はこの呼び出し（単調時計）。
//   void fandhe_apple_local_peer_stop(void)
//     終了時専用。専用キューへ同期ディスパッチして advertiser・browser・session を解放する。完了
//     コールバックは呼ばない。呼び出し元は Rust 側のロックを握らないこと。
//   bool fandhe_rust_local_peer_offer(request_id, peer_name, bytes, len)
//   void fandhe_rust_local_peer_finished(request_id, code)
//     Rust 側が公開する `extern "C"` 関数。offer が true を返したら受け取り成功（窓を閉じる）、
//     false なら不正・古い入力として相手を切り、窓の残りで次の相手を待つ。code: 0 成功（相手なしを含む）/ 1 許可なし /
//     2 今は使えない / 3 非対応 / 4 中断・失敗（接続後に受信できず期限切れを含む） / 5 サービス種別が未宣言。意味づけは Rust 側
//     `fandhe_device::local_peer::decode_code` に一本化する。1 要求につき finished は高々 1 回
//     （`FandhePeerClose` が唯一の発火点）。
//
// スレッド: 専用のシリアルキューだけで動かし、main queue は使わない（main 側が取るロックとの
// デッドロックを避ける、`device-bridge-contracts.md` 11 節）。MPC の delegate はどのスレッドからも
// 呼ばれうるため、受け口はすべて専用キューへ `dispatch_async` して、現在の request_id か確かめる。
// MPC の delegate は weak なので、セッション全体を global 変数が強参照で持つ（同 1 節）。
//
// iOS の背景遷移（Issue #512）: iOS は背景で MPC が止まる。何もしないと窓切れが「相手なし」
// （code 0 の成功）に化けるため、`UIApplicationDidEnterBackgroundNotification` で現在のセッションを
// 確定する（未受信は 4、受信済みで猶予中なら 0）。背景のまま開始された場合も 4 で確定する。
// 購読前から背景だった場合は通知が来ないため、初回購読時に main へ非同期で現在状態を読みに行く
// （待たない。専用キューは止めず、読み終えるまで開始処理だけ保留して古い NO を見ないようにする。
// stop は保留中の開始処理を捨てるので、同期停止は main に依存しない）。
// Inactive（ローカルネットワーク許可ダイアログ等）は購読しない。拾うと正常な許可を中断扱いにする
// （`device-bridge-contracts.md` 1 節）。購読トークンは static で強参照する（ARC で即解放しない）。
//
// 【推測】macOS で許可を拒否したときの `didNotStart*` のエラーコードは未確認。PolicyDenied(-65570) を
// 許可なし、NSNetServicesMissingRequiredConfigurationError(-72008) を未宣言として写し、分からない
// コードは 4 に倒す。確認は #509。
// 表示名・受信文字列を NSLog 等へ出力しない（security.md A09）。

#import <Foundation/Foundation.h>
#import <MultipeerConnectivity/MultipeerConnectivity.h>
#import <TargetConditionals.h>
#if TARGET_OS_IPHONE
#import <UIKit/UIKit.h>
#endif

extern bool fandhe_rust_local_peer_offer(uint64_t request_id, const char *peer_name, const uint8_t *bytes, size_t len);
extern void fandhe_rust_local_peer_finished(uint64_t request_id, int32_t code);

static NSString *const kFandheServiceType = @"fandhe-p2p"; // Rust 側 SERVICE_TYPE と同じ（15 文字以内）
static const NSUInteger kFandheMaxOutgoing = 200;          // Rust 側 MAX_TEXT_BYTES と同じ
static const NSInteger kFandhePolicyDenied = -65570;       // kDNSServiceErr_PolicyDenied
static const NSInteger kFandheMissingConfig = -72008;      // NSNetServicesMissingRequiredConfigurationError
static const int64_t kFandheCloseGraceMs = 500;            // 受信後、自分の送信が出し切れるまで待つ

@interface FandhePeerSession : NSObject <MCSessionDelegate, MCNearbyServiceAdvertiserDelegate, MCNearbyServiceBrowserDelegate>
@property(nonatomic) uint64_t requestId;
@property(nonatomic, copy) NSString *ownName;
@property(nonatomic, copy) NSData *outgoing;
@property(nonatomic) NSTimeInterval deadlineUptime;
@property(nonatomic, strong) MCPeerID *selfPeer;
@property(nonatomic, strong) MCSession *session;
@property(nonatomic, strong) MCNearbyServiceAdvertiser *advertiser;
@property(nonatomic, strong) MCNearbyServiceBrowser *browser;
@property(nonatomic, strong) MCPeerID *target; // 対象の相手。未定は nil
@property(nonatomic) BOOL sent;
@property(nonatomic) BOOL received;
@end

static dispatch_queue_t g_peer_queue;
static FandhePeerSession *g_peer_session = nil;

static dispatch_queue_t FandhePeerQueue(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      g_peer_queue = dispatch_queue_create("com.fandhe.local-peer", DISPATCH_QUEUE_SERIAL);
    });
    return g_peer_queue;
}

// 現在のセッションかを確かめて返す（古い・破棄済みなら nil）。
static FandhePeerSession *FandhePeerCurrent(uint64_t request_id) {
    FandhePeerSession *s = g_peer_session;
    return (s != nil && s.requestId == request_id) ? s : nil;
}

// セッションを止めて破棄する（専用キュー上でのみ呼ぶ。コールバックは出さない）。
// 途中まで作った状態でも、確保済みの advertiser・browser・session を全部解放する。
static void FandhePeerTeardown(void) {
    FandhePeerSession *s = g_peer_session;
    g_peer_session = nil;
    if (s == nil) {
        return;
    }
    if (s.advertiser != nil) {
        [s.advertiser stopAdvertisingPeer];
        s.advertiser.delegate = nil;
        s.advertiser = nil;
    }
    if (s.browser != nil) {
        [s.browser stopBrowsingForPeers];
        s.browser.delegate = nil;
        s.browser = nil;
    }
    if (s.session != nil) {
        [s.session disconnect];
        s.session.delegate = nil;
        s.session = nil;
    }
    s.target = nil;
}

// 完了の唯一の発火点（専用キュー上でのみ呼ぶ）。現在のセッションでなければ何もしない。
// teardown の後に finished を 1 回呼ぶ。
static void FandhePeerClose(uint64_t request_id, int32_t code) {
    if (FandhePeerCurrent(request_id) == nil) {
        return;
    }
    FandhePeerTeardown();
    fandhe_rust_local_peer_finished(request_id, code);
}

// 開始失敗のエラーを完了コードへ写す（Rust 側 decode_code の表）。
static int32_t FandhePeerCodeForError(NSError *error) {
    if (error.code == kFandhePolicyDenied) {
        return 1;
    }
    if (error.code == kFandheMissingConfig) {
        return 5;
    }
    return 4;
}

#if TARGET_OS_IPHONE
static id g_peer_bg_observer = nil;
static id g_peer_fg_observer = nil;
static BOOL g_peer_in_background = NO; // 専用キュー上でのみ読み書きする
static BOOL g_peer_state_known = NO;   // 背景状態が確定したか（専用キュー上でのみ）
static NSMutableArray<dispatch_block_t> *g_peer_pending = nil; // 確定待ちの開始処理（専用キュー上でのみ）

// 背景状態が確定した時点で、保留中の開始処理を順に流す。専用キュー上で呼ぶ。
static void FandhePeerStateKnown(void) {
    g_peer_state_known = YES;
    NSArray<dispatch_block_t> *p = g_peer_pending;
    g_peer_pending = nil;
    for (dispatch_block_t b in p) {
        b();
    }
}

// 背景遷移の購読を一度だけ張る。ブロックは専用キューへ dispatch_async するだけ（ロックなし・main 待ちなし）。
static void FandhePeerEnsureLifecycleObservers(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      // 購読前に既に背景だった場合は通知が来ない。現在状態を main で読み、専用キューへ戻して確定する。
      // 通知が先に届いて確定済みなら、古い読み取り値で上書きしない。
      dispatch_async(dispatch_get_main_queue(), ^{
        BOOL bg = [UIApplication sharedApplication].applicationState == UIApplicationStateBackground;
        dispatch_async(FandhePeerQueue(), ^{
          if (!g_peer_state_known) {
              g_peer_in_background = bg;
              FandhePeerStateKnown();
          }
        });
      });
      NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
      g_peer_bg_observer = [nc addObserverForName:UIApplicationDidEnterBackgroundNotification
                                           object:nil
                                            queue:nil
                                       usingBlock:^(NSNotification *__unused note) {
                                         dispatch_async(FandhePeerQueue(), ^{
                                           @autoreleasepool {
                                               g_peer_in_background = YES;
                                               if (!g_peer_state_known) {
                                                   FandhePeerStateKnown();
                                               }
                                               FandhePeerSession *s = g_peer_session;
                                               if (s != nil) {
                                                   FandhePeerClose(s.requestId, s.received ? 0 : 4);
                                               }
                                           }
                                         });
                                       }];
      g_peer_fg_observer = [nc addObserverForName:UIApplicationWillEnterForegroundNotification
                                           object:nil
                                            queue:nil
                                       usingBlock:^(NSNotification *__unused note) {
                                         dispatch_async(FandhePeerQueue(), ^{
                                           g_peer_in_background = NO;
                                           if (!g_peer_state_known) {
                                               FandhePeerStateKnown();
                                           }
                                         });
                                       }];
    });
}
#endif

// 相手の表示名が "fandhe-" + 小文字 hex 8 桁かの検査。SSOT は Rust 側 `is_valid_peer_name`
// （`fandhe_device::local_peer`）で、ここはそれと同じ形に揃えた前段チェック。厳密な検証は Rust 側 offer も行う。
static BOOL FandhePeerLooksValid(MCPeerID *peer) {
    NSString *n = peer.displayName;
    if (n.length != 15 || ![n hasPrefix:@"fandhe-"]) {
        return NO;
    }
    for (NSUInteger i = 7; i < 15; i++) {
        unichar c = [n characterAtIndex:i];
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'))) {
            return NO;
        }
    }
    return YES;
}

@implementation FandhePeerSession

// ---- MCNearbyServiceBrowserDelegate ----

- (void)browser:(MCNearbyServiceBrowser *)browser foundPeer:(MCPeerID *)peerID withDiscoveryInfo:(NSDictionary<NSString *, NSString *> *)info {
    uint64_t rid = self.requestId;
    dispatch_async(FandhePeerQueue(), ^{
      @autoreleasepool {
          FandhePeerSession *s = FandhePeerCurrent(rid);
          if (s != self || s.target != nil || !FandhePeerLooksValid(peerID) || [peerID isEqual:s.selfPeer]) {
              return;
          }
          // 招待する側は表示名が大きい方（Rust 側 should_invite と同じ規則）。
          if (strcmp(s.ownName.UTF8String, peerID.displayName.UTF8String) <= 0) {
              return;
          }
          NSTimeInterval remaining = s.deadlineUptime - [NSProcessInfo processInfo].systemUptime;
          if (remaining < 1.0) {
              remaining = 1.0;
          }
          s.target = peerID;
          [s.browser invitePeer:peerID toSession:s.session withContext:nil timeout:remaining];
      }
    });
}

// browse の lostPeer は無視する。招待中・承認後・接続中の進行は MCSession の状態（didChangeState）だけで
// 判断し、広告の消失で target を外さない（外すと後続の Connected / didReceiveData が捨てられる）。
- (void)browser:(MCNearbyServiceBrowser *)browser lostPeer:(MCPeerID *)peerID {
}

- (void)browser:(MCNearbyServiceBrowser *)browser didNotStartBrowsingForPeers:(NSError *)error {
    uint64_t rid = self.requestId;
    int32_t code = FandhePeerCodeForError(error);
    dispatch_async(FandhePeerQueue(), ^{
      @autoreleasepool {
          FandhePeerClose(rid, code);
      }
    });
}

// ---- MCNearbyServiceAdvertiserDelegate ----

- (void)advertiser:(MCNearbyServiceAdvertiser *)advertiser
    didReceiveInvitationFromPeer:(MCPeerID *)peerID
                     withContext:(NSData *)context
               invitationHandler:(void (^)(BOOL accept, MCSession *session))invitationHandler {
    uint64_t rid = self.requestId;
    dispatch_async(FandhePeerQueue(), ^{
      @autoreleasepool {
          // invitationHandler は必ず 1 回だけ呼ぶ（古い・対象外は拒否）。
          FandhePeerSession *s = FandhePeerCurrent(rid);
          if (s == self && s.target == nil && FandhePeerLooksValid(peerID) &&
              strcmp(s.ownName.UTF8String, peerID.displayName.UTF8String) < 0) {
              s.target = peerID;
              invitationHandler(YES, s.session);
          } else {
              invitationHandler(NO, nil);
          }
      }
    });
}

- (void)advertiser:(MCNearbyServiceAdvertiser *)advertiser didNotStartAdvertisingPeer:(NSError *)error {
    uint64_t rid = self.requestId;
    int32_t code = FandhePeerCodeForError(error);
    dispatch_async(FandhePeerQueue(), ^{
      @autoreleasepool {
          FandhePeerClose(rid, code);
      }
    });
}

// ---- MCSessionDelegate ----

- (void)session:(MCSession *)session peer:(MCPeerID *)peerID didChangeState:(MCSessionState)state {
    uint64_t rid = self.requestId;
    dispatch_async(FandhePeerQueue(), ^{
      @autoreleasepool {
          FandhePeerSession *s = FandhePeerCurrent(rid);
          if (s != self || s.target == nil || ![peerID isEqual:s.target]) {
              return;
          }
          if (state == MCSessionStateConnected && !s.sent) {
              NSError *err = nil;
              if ([s.session sendData:s.outgoing toPeers:@[ peerID ] withMode:MCSessionSendDataReliable error:&err]) {
                  s.sent = YES;
              } else {
                  FandhePeerClose(rid, 4); // 送れないのに成功扱いにしない
              }
          } else if (state == MCSessionStateNotConnected && !s.received) {
              if (s.sent) {
                  // 接続が成立し送信まで済んだのに、受け取る前に切れた。相手なしの成功にせず失敗で確定する。
                  FandhePeerClose(rid, 4);
              } else {
                  // 接続成立前に切れた（招待の拒否・失敗を含む）。次の相手を待つ。
                  s.target = nil;
              }
          }
      }
    });
}

- (void)session:(MCSession *)session didReceiveData:(NSData *)data fromPeer:(MCPeerID *)peerID {
    uint64_t rid = self.requestId;
    NSData *copy = [data copy];
    dispatch_async(FandhePeerQueue(), ^{
      @autoreleasepool {
          FandhePeerSession *s = FandhePeerCurrent(rid);
          if (s != self || s.received || s.target == nil || ![peerID isEqual:s.target]) {
              return;
          }
          if (fandhe_rust_local_peer_offer(rid, peerID.displayName.UTF8String, (const uint8_t *)copy.bytes, copy.length)) {
              s.received = YES;
              // 自分の送信が出し切れるよう少し待ってから閉じる（窓の期限でも閉じるので二重でも無害）。
              dispatch_after(dispatch_time(DISPATCH_TIME_NOW, kFandheCloseGraceMs * NSEC_PER_MSEC), FandhePeerQueue(), ^{
                @autoreleasepool {
                    FandhePeerClose(rid, 0);
                }
              });
          } else {
              // 不正・古い入力。対象を固定したままだと近傍の相手が窓を占有できるため、切って次の相手を待つ。
              // sent も戻し、次の正規の相手と改めて交換できるようにする。専用キュー上なので世代照合済み。
              MCPeerID *bad = s.target;
              s.target = nil;
              s.sent = NO;
              [s.session cancelConnectPeer:bad];
          }
      }
    });
}

// 相手を認証しない（セッションは暗号化必須だが、同じ種別を名乗る相手は区別しない。守らない範囲）。
// 実装した以上は handler を必ず 1 回呼ぶ。
- (void)session:(MCSession *)session
    didReceiveCertificate:(NSArray *)certificate
                 fromPeer:(MCPeerID *)peerID
       certificateHandler:(void (^)(BOOL accept))certificateHandler {
    certificateHandler(YES);
}

- (void)session:(MCSession *)session didReceiveStream:(NSInputStream *)stream withName:(NSString *)streamName fromPeer:(MCPeerID *)peerID {
    [stream close];
}

- (void)session:(MCSession *)session didStartReceivingResourceWithName:(NSString *)resourceName fromPeer:(MCPeerID *)peerID withProgress:(NSProgress *)progress {
}

- (void)session:(MCSession *)session didFinishReceivingResourceWithName:(NSString *)resourceName fromPeer:(MCPeerID *)peerID atURL:(NSURL *)localURL withError:(NSError *)error {
}

@end

int32_t fandhe_apple_local_peer_start(uint64_t request_id, const char *own_name, const uint8_t *outgoing, size_t outgoing_len, uint64_t window_ms) {
    if (own_name == NULL || outgoing == NULL || outgoing_len == 0 || outgoing_len > kFandheMaxOutgoing) {
        return 1;
    }
    // 引数は同期でコピーし、非同期処理へポインタを持ち越さない。
    NSString *own = [NSString stringWithUTF8String:own_name];
    NSData *out = [NSData dataWithBytes:outgoing length:outgoing_len];
    if (own == nil) {
        return 1;
    }
#if TARGET_OS_IPHONE
    FandhePeerEnsureLifecycleObservers();
#endif
    // 窓の起点はこの呼び出し（dispatch_time も systemUptime も単調時計）。
    dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW, (int64_t)(window_ms * NSEC_PER_MSEC));
    NSTimeInterval deadlineUptime = [NSProcessInfo processInfo].systemUptime + (NSTimeInterval)window_ms / 1000.0;
    dispatch_block_t work = ^{
      @autoreleasepool {
          // 進行中のセッションは置き換える（コールバックなし）。
          FandhePeerTeardown();

          FandhePeerSession *s = [[FandhePeerSession alloc] init];
          s.requestId = request_id;
          s.ownName = own;
          s.outgoing = out;
          s.deadlineUptime = deadlineUptime;
          g_peer_session = s; // 以降の失敗は Close が teardown する
#if TARGET_OS_IPHONE
          if (g_peer_in_background) {
              FandhePeerClose(request_id, 4); // 背景では MPC が動かない。相手なしの成功にしない
              return;
          }
#endif

          s.selfPeer = [[MCPeerID alloc] initWithDisplayName:own];
          s.session = [[MCSession alloc] initWithPeer:s.selfPeer securityIdentity:nil encryptionPreference:MCEncryptionRequired];
          s.advertiser = [[MCNearbyServiceAdvertiser alloc] initWithPeer:s.selfPeer discoveryInfo:nil serviceType:kFandheServiceType];
          s.browser = [[MCNearbyServiceBrowser alloc] initWithPeer:s.selfPeer serviceType:kFandheServiceType];
          if (s.selfPeer == nil || s.session == nil || s.advertiser == nil || s.browser == nil) {
              FandhePeerClose(request_id, 3);
              return;
          }
          s.session.delegate = s;
          s.advertiser.delegate = s;
          s.browser.delegate = s;
          [s.advertiser startAdvertisingPeer];
          [s.browser startBrowsingForPeers];

          dispatch_after(deadline, FandhePeerQueue(), ^{
            @autoreleasepool {
                // 接続・送信まで済んだのに受け取れず期限切れなら失敗（相手なしの成功にしない）。
                // 未接続のまま期限切れ（相手が見つからない）だけ 0。
                FandhePeerSession *cur = FandhePeerCurrent(request_id);
                FandhePeerClose(request_id, (cur != nil && cur.sent && !cur.received) ? 4 : 0);
            }
          });
      }
    };
    dispatch_async(FandhePeerQueue(), ^{
#if TARGET_OS_IPHONE
      if (!g_peer_state_known) {
          // 背景状態の確定待ち。stop が来れば g_peer_pending ごと捨てる。
          if (g_peer_pending == nil) {
              g_peer_pending = [NSMutableArray array];
          }
          [g_peer_pending addObject:work];
          return;
      }
#endif
      work();
    });
    return 0;
}

void fandhe_apple_local_peer_stop(void) {
    dispatch_sync(FandhePeerQueue(), ^{
      @autoreleasepool {
#if TARGET_OS_IPHONE
          g_peer_pending = nil; // 未投入の開始処理も無効化する
#endif
          FandhePeerTeardown();
      }
    });
}
