// Phase 11 の拡充ループ（親 #110）型サンプル「近くの端末との直接通信」: macOS（Issue #511）の実装。
// iOS（b-ios）も同じファイルを共用する。Foundation と MultipeerConnectivity だけを使い、
// AppKit / UIKit は使わない。
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
//     false なら不正・古い入力として捨てて待ち続ける。code: 0 成功（相手なしを含む）/ 1 許可なし /
//     2 今は使えない / 3 非対応 / 4 中断・失敗 / 5 サービス種別が未宣言。意味づけは Rust 側
//     `fandhe_device::local_peer::decode_code` に一本化する。1 要求につき finished は高々 1 回
//     （`FandhePeerClose` が唯一の発火点）。
//
// スレッド: 専用のシリアルキューだけで動かし、main queue は使わない（main 側が取るロックとの
// デッドロックを避ける、`device-bridge-contracts.md` 11 節）。MPC の delegate はどのスレッドからも
// 呼ばれうるため、受け口はすべて専用キューへ `dispatch_async` して、現在の request_id か確かめる。
// MPC の delegate は weak なので、セッション全体を global 変数が強参照で持つ（同 1 節）。
//
// 【推測】macOS で許可を拒否したときの `didNotStart*` のエラーコードは未確認。PolicyDenied(-65570) を
// 許可なし、NSNetServicesMissingRequiredConfigurationError(-72008) を未宣言として写し、分からない
// コードは 4 に倒す。確認は #509。
// 表示名・受信文字列を NSLog 等へ出力しない（security.md A09）。

#import <Foundation/Foundation.h>
#import <MultipeerConnectivity/MultipeerConnectivity.h>

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

// 相手の表示名が自分のアプリの形（"fandhe-" + 8 文字）かの前段チェック。厳密な検証は Rust 側 offer が行う。
static BOOL FandhePeerLooksValid(MCPeerID *peer) {
    NSString *n = peer.displayName;
    return n.length == 15 && [n hasPrefix:@"fandhe-"];
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
              // 受け取る前に切れた（招待の拒否・失敗を含む）。次の相手を待つ。
              s.target = nil;
              s.sent = NO;
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
          }
          // false は不正・古い入力。捨てて待ち続ける。
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
    // 窓の起点はこの呼び出し（dispatch_time も systemUptime も単調時計）。
    dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW, (int64_t)(window_ms * NSEC_PER_MSEC));
    NSTimeInterval deadlineUptime = [NSProcessInfo processInfo].systemUptime + (NSTimeInterval)window_ms / 1000.0;
    dispatch_async(FandhePeerQueue(), ^{
      @autoreleasepool {
          // 進行中のセッションは置き換える（コールバックなし）。
          FandhePeerTeardown();

          FandhePeerSession *s = [[FandhePeerSession alloc] init];
          s.requestId = request_id;
          s.ownName = own;
          s.outgoing = out;
          s.deadlineUptime = deadlineUptime;
          g_peer_session = s; // 以降の失敗は Close が teardown する

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
                FandhePeerClose(request_id, 0);
            }
          });
      }
    });
    return 0;
}

void fandhe_apple_local_peer_stop(void) {
    dispatch_sync(FandhePeerQueue(), ^{
      @autoreleasepool {
          FandhePeerTeardown();
      }
    });
}
