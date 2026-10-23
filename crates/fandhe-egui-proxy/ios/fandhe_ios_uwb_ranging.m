// UWB による相手までの距離: iOS 実装（Issue #654、親 #129、対応表 `uwb-ranging`）。
// NearbyInteraction の `NISession` + `NINearbyPeerConfiguration` で距離を測り、相手の discovery token の
// 交換に MultipeerConnectivity（MPC）を使う。最新の状態を static に保持する pull 型
// （`activity_recognition` と同型）。範囲は UWB 搭載 iPhone 2 台・同じアプリ同士・前面の間・距離のみ。
//
// `src/device/ios_uwb_ranging.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の feature `device`
// 有効時のみ `cc` でコンパイルし、NearbyInteraction をリンクする（MPC は local_peer 用にリンク済み）。
//
// 契約:
//   int32_t fandhe_ios_uwb_ranging_capabilities(int32_t *distance, int32_t *direction)
//       0 = 成功（out へ書く。値は 0 No / 1 Yes / 2 Unknown）、3 = 失敗（NULL）。許可は要らず同期で軽い。
//   int32_t fandhe_ios_uwb_ranging_start(void)
//       0 = 受理（冪等）、1 = 非搭載（距離 No。シミュレータはここ。何も確保しない）、
//       3 = 失敗（main 以外からの呼び出しを含む）。
//   int32_t fandhe_ios_uwb_ranging_read(float *distance_m, uint64_t *seq)
//       0 探索中 / 1 token 交換中 / 2 許可ダイアログ待ち / 3 距離あり（out へ書く）/ 4 距離なし /
//       5 相手の終了 / 6 中断 / 10 権限拒否 / 11 非搭載 / 12 失敗。3 以外では out を書き換えない。
//       NULL は 12。未開始・停止後は 0。終端（5 / 6 / 10 / 11 / 12）は stop まで同じ値を返す。
//   void fandhe_ios_uwb_ranging_stop(void)
//       冪等・失敗しない・すぐ戻る。状態を捨て、MPC と NISession の解放は serial queue へ非同期に送る。
//   Rust 側（`#[no_mangle]`）:
//       void fandhe_rust_uwb_ranging_changed(void)        状態を確定した直後（ロックの外）に再描画を要求する
//       bool fandhe_rust_uwb_ranging_token_len_ok(size_t) 相手の token の長さ検査（上限の SSOT は Rust 側）
//
// 流れ: start は世代を進めて探索中にし、準備（NISession 作成・MPC の advertise と browse）を serial queue で
// 行う。最初に見つけた 1 台だけと暗号化必須（MCEncryptionRequired）のセッションを張る。招待側は表示名の
// バイト順で決める（`fandhe_apple_local_peer.m` と同じ規則）。接続したら自分の token を送り、相手の token は
// 長さを見てから secure coding で `NIDiscoveryToken` 1 クラスだけを解凍する。解凍できない token は捨てて交換中の
// まま待つ。解凍できたら `runWithConfiguration:` で測距を始める。測距前に MPC が切れたら相手を捨てて探索へ戻す。
// 距離が `NINearbyObjectDistanceNotAvailable` なら「距離なし」（前回値を捨てる）。距離の範囲検証は Rust driver
// の 1 か所だけで、ここでは丸めない。表示名は端末名を使わずランダムにする（相手へ端末名を出さない）。
//
// 世代とスレッド: 状態は os_unfair_lock で守り、ロック内で Rust・OS を呼ばない。OS オブジェクトは serial queue
// 上の static が強参照で持つ（MPC の delegate は weak）。delegate の通知はすべて serial queue へ寄せ、
// 世代を照合して旧世代分を捨てる。main へ `dispatch_sync` しない（`device-bridge-contracts.md`「11」）。
// 期限は driver の単調時計だけが持ち、ここでは時間を測らない。許可ダイアログ（Inactive）の間、最初の距離が来るまでは
// 2（許可待ち）を返し driver の窓を止める。Inactive は中断扱いにしない。背景遷移（DidEnterBackground）では
// 資源を解放して 6 にし、自動では再開しない。購読トークンは static で強参照する。
//
// 守る範囲: 嘘の値を出さない・二重に確保しない・停止と中断と失敗の全経路で解放する・旧世代の通知を捨てる。
// 守らない範囲: 相手の認証、3 台以上での選び方、精度と更新間隔、背景継続、複数プロセス。
// 既知の天井: `timeout` 理由の再 run、許可拒否時の挙動、窓の長さは実機未確認（#648）。
// ログは出さない（距離・token・表示名・NSError のコードを出さない。security.md A09）。

#import <Foundation/Foundation.h>
#import <MultipeerConnectivity/MultipeerConnectivity.h>
#import <NearbyInteraction/NearbyInteraction.h>
#import <UIKit/UIKit.h>
#import <os/lock.h>

extern void fandhe_rust_uwb_ranging_changed(void);
extern bool fandhe_rust_uwb_ranging_token_len_ok(size_t len);

// Rust 側 `fandhe_device::uwb_ranging::SERVICE_TYPE` と同じ（15 文字以内）。
static NSString *const kServiceType = @"fandhe-uwb";
static const NSInteger kPolicyDenied = -65570; // kDNSServiceErr_PolicyDenied（ローカルネットワーク拒否）
static const NSTimeInterval kInviteTimeout = 30.0;

enum {
    kPhaseSearching = 0,
    kPhaseExchanging = 1,
    kPhaseMeasured = 3,
    kPhaseNoMeasurement = 4,
    kPhasePeerEnded = 5,
    kPhaseInterrupted = 6,
};

// ---- ロックで守る状態 ----
static os_unfair_lock gLock = OS_UNFAIR_LOCK_INIT;
static BOOL gActive = NO;
static uint64_t gEpoch = 0;
static int32_t gPhase = kPhaseSearching;
static int32_t gFailure = 0; // 0 = なし、10 / 11 / 12
static float gDistance = 0;
static uint64_t gSeq = 0; // プロセス内で単調増加。巻き戻さない
static BOOL gEverMeasured = NO;
static BOOL gInactive = NO;

static dispatch_queue_t Queue(void) {
    static dispatch_queue_t q;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        q = dispatch_queue_create("com.fandhe.uwb-ranging", DISPATCH_QUEUE_SERIAL);
    });
    return q;
}

static BOOL IsTerminalLocked(void) {
    return gFailure != 0 || gPhase == kPhasePeerEnded || gPhase == kPhaseInterrupted;
}

// 現在の世代なら状態を更新して YES。失敗・終端が確定済みなら上書きしない。ロックの外で呼ぶ。
// failure != 0 は失敗の確定、そうでなければ phase の更新（Measured のとき distance と seq を進める）。
static BOOL Update(uint64_t epoch, int32_t phase, int32_t failure, float distance) {
    os_unfair_lock_lock(&gLock);
    BOOL ok = gActive && epoch == gEpoch && !IsTerminalLocked();
    if (ok) {
        if (failure != 0) {
            gFailure = failure;
        } else {
            gPhase = phase;
            if (phase == kPhaseMeasured) {
                gDistance = distance;
                gSeq += 1;
                gEverMeasured = YES;
            }
        }
    }
    os_unfair_lock_unlock(&gLock);
    if (ok) {
        fandhe_rust_uwb_ranging_changed();
    }
    return ok;
}

static BOOL EpochCurrent(uint64_t epoch) {
    os_unfair_lock_lock(&gLock);
    BOOL ok = gActive && epoch == gEpoch && !IsTerminalLocked();
    os_unfair_lock_unlock(&gLock);
    return ok;
}

// ---- serial queue 上でだけ触る OS オブジェクト ----
@interface FandheUwbHandler : NSObject <MCSessionDelegate, MCNearbyServiceAdvertiserDelegate,
                                        MCNearbyServiceBrowserDelegate, NISessionDelegate>
@property(nonatomic) uint64_t epoch;
@end

static FandheUwbHandler *gHandler = nil;
static MCPeerID *gSelfPeer = nil;
static MCSession *gMcSession = nil;
static MCNearbyServiceAdvertiser *gAdvertiser = nil;
static MCNearbyServiceBrowser *gBrowser = nil;
static MCPeerID *gMcPeer = nil; // 選んだ相手。未定は nil
static NISession *gNi = nil;
static NIDiscoveryToken *gPeerToken = nil;
static BOOL gRanging = NO;

static void StopDiscovery(void) {
    if (gAdvertiser != nil) {
        [gAdvertiser stopAdvertisingPeer];
        gAdvertiser.delegate = nil;
        gAdvertiser = nil;
    }
    if (gBrowser != nil) {
        [gBrowser stopBrowsingForPeers];
        gBrowser.delegate = nil;
        gBrowser = nil;
    }
}

static void TeardownMpc(void) {
    StopDiscovery();
    if (gMcSession != nil) {
        [gMcSession disconnect];
        gMcSession.delegate = nil;
        gMcSession = nil;
    }
    gSelfPeer = nil;
    gMcPeer = nil;
}

// 確保済みの全資源を解放する（serial queue 上）。途中まで作った状態でも安全。
static void Teardown(void) {
    TeardownMpc();
    if (gNi != nil) {
        gNi.delegate = nil; // 自分の invalidate による通知を受けない
        [gNi invalidate];
        gNi = nil;
    }
    gPeerToken = nil;
    gRanging = NO;
    gHandler = nil;
}

// 失敗を確定して解放する（serial queue 上）。
static void FailAndRelease(uint64_t epoch, int32_t failure) {
    BOOL ok = Update(epoch, 0, failure, 0);
    if (ok) {
        Teardown();
    }
}

static int32_t FailureForMpcError(NSError *error) {
    return error.code == kPolicyDenied ? 10 : 12;
}

// MPC 一式を作って advertise と browse を始める（serial queue 上）。
static BOOL BuildMpc(FandheUwbHandler *h) {
    NSString *name = [[NSUUID UUID] UUIDString]; // 端末名は使わない
    gSelfPeer = [[MCPeerID alloc] initWithDisplayName:name];
    gMcSession = [[MCSession alloc] initWithPeer:gSelfPeer
                                securityIdentity:nil
                            encryptionPreference:MCEncryptionRequired];
    gMcSession.delegate = h;
    gAdvertiser = [[MCNearbyServiceAdvertiser alloc] initWithPeer:gSelfPeer
                                                    discoveryInfo:nil
                                                      serviceType:kServiceType];
    gAdvertiser.delegate = h;
    gBrowser = [[MCNearbyServiceBrowser alloc] initWithPeer:gSelfPeer serviceType:kServiceType];
    gBrowser.delegate = h;
    [gAdvertiser startAdvertisingPeer];
    [gBrowser startBrowsingForPeers];
    return YES;
}

static void Prepare(uint64_t epoch) {
    if (!EpochCurrent(epoch)) {
        return;
    }
    Teardown();
    NISession *ni = [[NISession alloc] init];
    if (ni.discoveryToken == nil) {
        [ni invalidate];
        Update(epoch, 0, 12, 0);
        return;
    }
    FandheUwbHandler *h = [[FandheUwbHandler alloc] init];
    h.epoch = epoch;
    gHandler = h;
    gNi = ni;
    BuildMpc(h);
}

// 相手の token で測距を始める（serial queue 上）。
static void StartRanging(FandheUwbHandler *h, NIDiscoveryToken *token) {
    NINearbyPeerConfiguration *cfg = [[NINearbyPeerConfiguration alloc] initWithPeerToken:token];
    gPeerToken = token;
    gNi.delegate = h;
    gRanging = YES;
    if (!Update(h.epoch, kPhaseNoMeasurement, 0, 0)) {
        return;
    }
    [gNi runWithConfiguration:cfg];
}

static BOOL TokenIsPeer(NSArray<NINearbyObject *> *objects) {
    for (NINearbyObject *o in objects) {
        if (gPeerToken != nil && [o.discoveryToken isEqual:gPeerToken]) {
            return YES;
        }
    }
    return NO;
}

@implementation FandheUwbHandler

// ---- MCSessionDelegate ----
- (void)session:(MCSession *)session peer:(MCPeerID *)peerID didChangeState:(MCSessionState)state {
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self != gHandler || session != gMcSession || ![peerID isEqual:gMcPeer]) {
                return;
            }
            if (state == MCSessionStateConnected && !gRanging) {
                if (!Update(self.epoch, kPhaseExchanging, 0, 0)) {
                    return;
                }
                NSError *err = nil;
                NSData *mine = [NSKeyedArchiver archivedDataWithRootObject:gNi.discoveryToken
                                                     requiringSecureCoding:YES
                                                                     error:&err];
                if (mine == nil ||
                    ![gMcSession sendData:mine toPeers:@[ peerID ] withMode:MCSessionSendDataReliable error:&err]) {
                    FailAndRelease(self.epoch, 12);
                }
            } else if (state == MCSessionStateNotConnected && !gRanging) {
                // 測距前に切れた: 相手を捨てて探索へ戻す。測距開始後の切断は無視する（終了は NI の peerEnded で拾う）
                TeardownMpc();
                if (Update(self.epoch, kPhaseSearching, 0, 0)) {
                    BuildMpc(self);
                }
            }
        }
    });
}

- (void)session:(MCSession *)session didReceiveData:(NSData *)data fromPeer:(MCPeerID *)peerID {
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self != gHandler || session != gMcSession || ![peerID isEqual:gMcPeer] || gRanging) {
                return;
            }
            if (!fandhe_rust_uwb_ranging_token_len_ok(data.length)) {
                return; // 非信頼入力。長さが不正なら解凍せず捨てる
            }
            NSError *err = nil;
            NIDiscoveryToken *token = [NSKeyedUnarchiver unarchivedObjectOfClass:[NIDiscoveryToken class]
                                                                        fromData:data
                                                                           error:&err];
            if (token == nil) {
                return; // 解凍できない token は捨てて交換中のまま待つ
            }
            StartRanging(self, token);
        }
    });
}

- (void)session:(MCSession *)session didReceiveStream:(NSInputStream *)stream withName:(NSString *)streamName fromPeer:(MCPeerID *)peerID {
}
- (void)session:(MCSession *)session didStartReceivingResourceWithName:(NSString *)resourceName fromPeer:(MCPeerID *)peerID withProgress:(NSProgress *)progress {
}
- (void)session:(MCSession *)session didFinishReceivingResourceWithName:(NSString *)resourceName fromPeer:(MCPeerID *)peerID atURL:(NSURL *)localURL withError:(NSError *)error {
}

// ---- MCNearbyServiceAdvertiserDelegate ----
- (void)advertiser:(MCNearbyServiceAdvertiser *)advertiser
    didReceiveInvitationFromPeer:(MCPeerID *)peerID
                     withContext:(NSData *)context
               invitationHandler:(void (^)(BOOL, MCSession *))invitationHandler {
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self != gHandler || advertiser != gAdvertiser || gMcPeer != nil || gMcSession == nil) {
                invitationHandler(NO, nil); // 相手は最初の 1 台だけ
                return;
            }
            gMcPeer = peerID;
            StopDiscovery();
            invitationHandler(YES, gMcSession);
        }
    });
}

- (void)advertiser:(MCNearbyServiceAdvertiser *)advertiser didNotStartAdvertisingPeer:(NSError *)error {
    int32_t f = FailureForMpcError(error);
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self == gHandler && advertiser == gAdvertiser) {
                FailAndRelease(self.epoch, f);
            }
        }
    });
}

// ---- MCNearbyServiceBrowserDelegate ----
- (void)browser:(MCNearbyServiceBrowser *)browser foundPeer:(MCPeerID *)peerID withDiscoveryInfo:(NSDictionary<NSString *, NSString *> *)info {
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self != gHandler || browser != gBrowser || gMcPeer != nil || gSelfPeer == nil) {
                return;
            }
            // 招待する側は表示名が大きい方（Rust 側 `should_invite` と同じ規則）。小さい側は招待を待つ。
            if (strcmp(gSelfPeer.displayName.UTF8String, peerID.displayName.UTF8String) <= 0) {
                return;
            }
            gMcPeer = peerID;
            MCSession *s = gMcSession;
            // 招待を送ってから停止する。先に停止すると browser が招待を届けられない。
            // 失敗・タイムアウト・キャンセルは Teardown 経由で StopDiscovery と gMcPeer の復元に至る。
            [browser invitePeer:peerID toSession:s withContext:nil timeout:kInviteTimeout];
            StopDiscovery();
        }
    });
}

- (void)browser:(MCNearbyServiceBrowser *)browser lostPeer:(MCPeerID *)peerID {
}

- (void)browser:(MCNearbyServiceBrowser *)browser didNotStartBrowsingForPeers:(NSError *)error {
    int32_t f = FailureForMpcError(error);
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self == gHandler && browser == gBrowser) {
                FailAndRelease(self.epoch, f);
            }
        }
    });
}

// ---- NISessionDelegate（main などから届くので serial queue へ寄せる）----
- (void)session:(NISession *)session didUpdateNearbyObjects:(NSArray<NINearbyObject *> *)nearbyObjects {
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self != gHandler || session != gNi || !gRanging) {
                return;
            }
            for (NINearbyObject *o in nearbyObjects) {
                if (gPeerToken == nil || ![o.discoveryToken isEqual:gPeerToken]) {
                    continue;
                }
                if (o.distance == NINearbyObjectDistanceNotAvailable) {
                    Update(self.epoch, kPhaseNoMeasurement, 0, 0); // 前回値を捨てる
                } else {
                    Update(self.epoch, kPhaseMeasured, 0, o.distance); // 生の値。検証は Rust driver
                }
                return;
            }
        }
    });
}

- (void)session:(NISession *)session didRemoveNearbyObjects:(NSArray<NINearbyObject *> *)nearbyObjects withReason:(NINearbyObjectRemovalReason)reason {
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self != gHandler || session != gNi || !gRanging || !TokenIsPeer(nearbyObjects)) {
                return;
            }
            if (reason == NINearbyObjectRemovalReasonPeerEnded) {
                if (Update(self.epoch, kPhasePeerEnded, 0, 0)) {
                    Teardown();
                }
            } else {
                // timeout: 距離なしにして同じ設定で測距をやり直す
                if (Update(self.epoch, kPhaseNoMeasurement, 0, 0) && gPeerToken != nil) {
                    [gNi runWithConfiguration:[[NINearbyPeerConfiguration alloc] initWithPeerToken:gPeerToken]];
                }
            }
        }
    });
}

- (void)sessionWasSuspended:(NISession *)session {
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self != gHandler || session != gNi) {
                return;
            }
            if (Update(self.epoch, kPhaseInterrupted, 0, 0)) {
                Teardown(); // 自動では再開しない
            }
        }
    });
}

- (void)sessionSuspensionEnded:(NISession *)session {
}

- (void)session:(NISession *)session didInvalidateWithError:(NSError *)error {
    int32_t f = 12;
    if ([error.domain isEqualToString:NIErrorDomain]) {
        if (error.code == NIErrorCodeUserDidNotAllow) {
            f = 10;
        } else if (error.code == NIErrorCodeUnsupportedPlatform) {
            f = 11;
        }
    }
    dispatch_async(Queue(), ^{
        @autoreleasepool {
            if (self == gHandler && session == gNi) {
                FailAndRelease(self.epoch, f);
            }
        }
    });
}

@end

// ---- ライフサイクル（main で初回 start 時に購読。トークンはプロセス存続中保持）----
static id gBgToken = nil;
static id gResignToken = nil;
static id gActiveToken = nil;

static void EnsureObservers(void) {
    if (gBgToken != nil) {
        return;
    }
    NSNotificationCenter *nc = NSNotificationCenter.defaultCenter;
    gBgToken = [nc addObserverForName:UIApplicationDidEnterBackgroundNotification
                               object:nil
                                queue:nil
                           usingBlock:^(NSNotification *__unused n) {
        os_unfair_lock_lock(&gLock);
        BOOL changed = NO;
        if (gActive && !IsTerminalLocked()) {
            gPhase = kPhaseInterrupted;
            gEpoch += 1; // 旧世代の通知はここから捨てる
            changed = YES;
        }
        os_unfair_lock_unlock(&gLock);
        if (changed) {
            dispatch_async(Queue(), ^{
                @autoreleasepool {
                    Teardown();
                }
            });
            fandhe_rust_uwb_ranging_changed();
        }
    }];
    // Inactive（許可ダイアログ・コントロールセンター）は中断にしない。フラグだけ持つ。
    gResignToken = [nc addObserverForName:UIApplicationWillResignActiveNotification
                                   object:nil
                                    queue:nil
                               usingBlock:^(NSNotification *__unused n) {
        os_unfair_lock_lock(&gLock);
        gInactive = YES;
        os_unfair_lock_unlock(&gLock);
        fandhe_rust_uwb_ranging_changed();
    }];
    gActiveToken = [nc addObserverForName:UIApplicationDidBecomeActiveNotification
                                   object:nil
                                    queue:nil
                               usingBlock:^(NSNotification *__unused n) {
        os_unfair_lock_lock(&gLock);
        gInactive = NO;
        os_unfair_lock_unlock(&gLock);
        fandhe_rust_uwb_ranging_changed();
    }];
}

// ---- C ABI ----
int32_t fandhe_ios_uwb_ranging_capabilities(int32_t *distance, int32_t *direction) {
    if (distance == NULL || direction == NULL) {
        return 3;
    }
    @autoreleasepool {
        if (@available(iOS 16.0, *)) {
            id<NIDeviceCapability> cap = NISession.deviceCapabilities;
            *distance = cap.supportsPreciseDistanceMeasurement ? 1 : 0;
            *direction = cap.supportsDirectionMeasurement ? 1 : 0;
        } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            *distance = NISession.isSupported ? 1 : 0;
#pragma clang diagnostic pop
            *direction = 2; // iOS 16 未満は判定できない。Yes へ畳まない
        }
        return 0;
    }
}

int32_t fandhe_ios_uwb_ranging_start(void) {
    if (![NSThread isMainThread]) {
        return 3;
    }
    @autoreleasepool {
        int32_t d = 0, dir = 0;
        if (fandhe_ios_uwb_ranging_capabilities(&d, &dir) != 0) {
            return 3;
        }
        if (d != 1) {
            return 1;
        }
        EnsureObservers();
        UIApplicationState st = UIApplication.sharedApplication.applicationState;
        os_unfair_lock_lock(&gLock);
        if (gActive) {
            os_unfair_lock_unlock(&gLock);
            return 0;
        }
        gActive = YES;
        gEpoch += 1;
        uint64_t epoch = gEpoch;
        gPhase = kPhaseSearching;
        gFailure = 0;
        gDistance = 0;
        gEverMeasured = NO;
        gInactive = st == UIApplicationStateInactive;
        BOOL background = st == UIApplicationStateBackground;
        if (background) {
            gPhase = kPhaseInterrupted; // 背景のまま開始されたら資源を確保せず中断で確定する
        }
        os_unfair_lock_unlock(&gLock);
        if (background) {
            fandhe_rust_uwb_ranging_changed();
        } else {
            dispatch_async(Queue(), ^{
                @autoreleasepool {
                    Prepare(epoch);
                }
            });
        }
        return 0;
    }
}

int32_t fandhe_ios_uwb_ranging_read(float *distance_m, uint64_t *seq) {
    if (distance_m == NULL || seq == NULL) {
        return 12;
    }
    int32_t rc;
    os_unfair_lock_lock(&gLock);
    if (!gActive) {
        rc = 0;
    } else if (gFailure != 0) {
        rc = gFailure;
    } else if (gPhase == kPhasePeerEnded || gPhase == kPhaseInterrupted) {
        rc = gPhase;
    } else if (gInactive && !gEverMeasured) {
        rc = 2; // 許可ダイアログ待ちは期限に数えない
    } else if (gPhase == kPhaseMeasured) {
        *distance_m = gDistance;
        *seq = gSeq;
        rc = 3;
    } else {
        rc = gPhase;
    }
    os_unfair_lock_unlock(&gLock);
    return rc;
}

void fandhe_ios_uwb_ranging_stop(void) {
    os_unfair_lock_lock(&gLock);
    BOOL was = gActive;
    gActive = NO;
    gEpoch += 1; // 以降に届く通知は世代照合で捨てる
    gPhase = kPhaseSearching;
    gFailure = 0;
    gDistance = 0;
    gEverMeasured = NO;
    os_unfair_lock_unlock(&gLock);
    if (was) {
        // 解放は serial queue へ非同期に送る。次の start の準備はその後ろに並ぶ
        dispatch_async(Queue(), ^{
            @autoreleasepool {
                Teardown();
            }
        });
    }
}
