// 通話 UI 統合: iOS 実装（Issue #277、親 #87）。CallKit の `CXProvider` で疑似着信を 1 件出し、
// 応答・拒否・終了の結果を Rust へ返す。`src/device/ios_call_ui.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS 分岐の feature `device` 有効時のみ `cc` コンパイルし CallKit をリンクする。
//
// 契約:
//   fandhe_ios_call_ui_report(uint64_t call_id)  何スレッドからでも可。main queue へ非同期に渡す
//   fandhe_ios_call_ui_end(uint64_t call_id)     同上。冪等。id が現在の通話と一致しなければ何もしない
//   Rust の fandhe_rust_call_ui_event(call_id, kind, domain, code) を main queue から呼ぶ。
//     kind: 1 Ringing / 2 Answered / 3 Declined / 4 RemoteEnded / 5 報告エラー（domain・code つき）/
//           6 providerDidReset / 7 action のタイムアウト
//     domain: 1 = CXErrorDomainIncomingCall、0 = それ以外。code は NSError.code の生値
//   同期で Rust へ通知を返さない（ドライバのロックと再入しない）。`dispatch_sync` は使わない。
//
// スレッド: 状態（現在の通話 1 件）は main queue だけで触る。ロックは不要。provider / delegate / controller は
// static で強参照する（delegate は弱参照で保持されるため）。
// 嘘の成功を出さない: Ringing は completion が error なしで、かつ現在の通話のときだけ送る。取り消し・差し替え後に
// 遅れて届いた受理は通知せず OS 側で閉じる。Active の終了要求が失敗したら reportCall で必ず閉じる。
// 通話履歴には残さない（includesCallsInRecents = NO）。AVAudioSession には触れない。
// 守る範囲: 1 プロセス内の順序、嘘の成功の禁止、CXCall の解放。
// 守らない範囲: 背景での通話の継続、プロセス死、複数通話、Bluetooth・車載機器の表示。
// 既知の天井: シミュレータでの挙動と unentitled が出るかは実機で確認する（#275）。
// ログは出さない（NSError・UUID・call_id を出さない。security.md A09）。

#import <AVFoundation/AVFoundation.h>
#import <CallKit/CallKit.h>
#import <Foundation/Foundation.h>

extern void fandhe_rust_call_ui_event(uint64_t call_id, int32_t kind, int32_t domain, int64_t code);

typedef NS_ENUM(NSInteger, FandheCallStage) {
    FandheCallStageReporting,
    FandheCallStageRinging,
    FandheCallStageActive,
};

static uint64_t gCurrentId = 0; // 0 = なし
static NSUUID *gCurrentUUID = nil;
static FandheCallStage gStage = FandheCallStageReporting;

static CXProvider *gProvider = nil;
static CXCallController *gController = nil;
static id<CXProviderDelegate> gDelegate = nil;

static void clearCurrent(void) {
    gCurrentId = 0;
    gCurrentUUID = nil;
}

@interface FandheCallDelegate : NSObject <CXProviderDelegate>
@end

@implementation FandheCallDelegate

- (void)providerDidReset:(CXProvider *)provider {
    if (gCurrentId != 0) {
        uint64_t cid = gCurrentId;
        clearCurrent();
        fandhe_rust_call_ui_event(cid, 6, 0, 0);
    }
}

- (void)provider:(CXProvider *)provider performAnswerCallAction:(CXAnswerCallAction *)action {
    if (gCurrentId != 0 && [action.callUUID isEqual:gCurrentUUID] && gStage == FandheCallStageRinging) {
        [action fulfill];
        gStage = FandheCallStageActive;
        fandhe_rust_call_ui_event(gCurrentId, 2, 0, 0);
    } else {
        [action fail];
        [provider reportCallWithUUID:action.callUUID endedAtDate:nil reason:CXCallEndedReasonFailed];
    }
}

- (void)provider:(CXProvider *)provider performEndCallAction:(CXEndCallAction *)action {
    [action fulfill];
    // 自分で要求した終了（end）は先に現在の通話を空にしてあるため、ここでは通知しない。
    if (gCurrentId != 0 && [action.callUUID isEqual:gCurrentUUID]) {
        uint64_t cid = gCurrentId;
        BOOL ringing = (gStage == FandheCallStageRinging);
        clearCurrent();
        fandhe_rust_call_ui_event(cid, ringing ? 3 : 4, 0, 0);
    }
}

// 音声・保留・ミュート・グループ・DTMF は扱わない。成功にしない。
- (void)provider:(CXProvider *)provider performSetHeldCallAction:(CXSetHeldCallAction *)action { [action fail]; }
- (void)provider:(CXProvider *)provider performSetMutedCallAction:(CXSetMutedCallAction *)action { [action fail]; }
- (void)provider:(CXProvider *)provider performSetGroupCallAction:(CXSetGroupCallAction *)action { [action fail]; }
- (void)provider:(CXProvider *)provider performPlayDTMFCallAction:(CXPlayDTMFCallAction *)action { [action fail]; }

- (void)provider:(CXProvider *)provider timedOutPerformingAction:(CXAction *)action {
    if (gCurrentId != 0) {
        uint64_t cid = gCurrentId;
        NSUUID *uuid = gCurrentUUID;
        clearCurrent();
        [provider reportCallWithUUID:uuid endedAtDate:nil reason:CXCallEndedReasonFailed];
        fandhe_rust_call_ui_event(cid, 7, 0, 0);
    }
}

- (void)provider:(CXProvider *)provider didActivateAudioSession:(AVAudioSession *)audioSession {}
- (void)provider:(CXProvider *)provider didDeactivateAudioSession:(AVAudioSession *)audioSession {}

@end

static void ensureProvider(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        CXProviderConfiguration *cfg = [[CXProviderConfiguration alloc] init];
        cfg.supportsVideo = NO;
        cfg.maximumCallGroups = 1;
        cfg.maximumCallsPerCallGroup = 1;
        cfg.supportedHandleTypes = [NSSet setWithObject:@(CXHandleTypeGeneric)];
        cfg.includesCallsInRecents = NO;
        gDelegate = [[FandheCallDelegate alloc] init];
        gProvider = [[CXProvider alloc] initWithConfiguration:cfg];
        [gProvider setDelegate:gDelegate queue:nil];
        gController = [[CXCallController alloc] init];
    });
}

static void doReport(uint64_t cid) {
    ensureProvider();
    if (gCurrentUUID != nil) {
        [gProvider reportCallWithUUID:gCurrentUUID endedAtDate:nil reason:CXCallEndedReasonRemoteEnded];
    }
    NSUUID *uuid = [NSUUID UUID];
    gCurrentId = cid;
    gCurrentUUID = uuid;
    gStage = FandheCallStageReporting;

    CXCallUpdate *update = [[CXCallUpdate alloc] init];
    update.localizedCallerName = @"Fandhe";
    update.remoteHandle = [[CXHandle alloc] initWithType:CXHandleTypeGeneric value:@"fandhe-demo"];
    update.hasVideo = NO;
    update.supportsHolding = NO;
    update.supportsGrouping = NO;
    update.supportsUngrouping = NO;
    update.supportsDTMF = NO;

    [gProvider reportNewIncomingCallWithUUID:uuid update:update completion:^(NSError *_Nullable error) {
        // delegate queue = main。
        BOOL current = (gCurrentUUID != nil && [gCurrentUUID isEqual:uuid]);
        if (error == nil) {
            if (current) {
                gStage = FandheCallStageRinging;
                fandhe_rust_call_ui_event(cid, 1, 0, 0);
            } else {
                // 取り消し・差し替え済み。通知せず OS 側で閉じる。
                [gProvider reportCallWithUUID:uuid endedAtDate:nil reason:CXCallEndedReasonUnanswered];
            }
        } else if (current) {
            int32_t domain = [error.domain isEqualToString:CXErrorDomainIncomingCall] ? 1 : 0;
            clearCurrent();
            fandhe_rust_call_ui_event(cid, 5, domain, (int64_t)error.code);
        }
    }];
}

static void doEnd(uint64_t cid) {
    if (cid == 0 || gCurrentId != cid) {
        return;
    }
    ensureProvider();
    NSUUID *uuid = gCurrentUUID;
    FandheCallStage stage = gStage;
    clearCurrent();
    switch (stage) {
        case FandheCallStageReporting:
            // 遅れて届く成功の completion が閉じる。
            break;
        case FandheCallStageRinging:
            [gProvider reportCallWithUUID:uuid endedAtDate:nil reason:CXCallEndedReasonUnanswered];
            break;
        case FandheCallStageActive: {
            CXTransaction *t = [[CXTransaction alloc] initWithAction:[[CXEndCallAction alloc] initWithCallUUID:uuid]];
            [gController requestTransaction:t completion:^(NSError *_Nullable error) {
                if (error != nil) {
                    [gProvider reportCallWithUUID:uuid endedAtDate:nil reason:CXCallEndedReasonFailed];
                }
            }];
            break;
        }
    }
}

void fandhe_ios_call_ui_report(uint64_t call_id) {
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool { doReport(call_id); }
    });
}

void fandhe_ios_call_ui_end(uint64_t call_id) {
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool { doEnd(call_id); }
    });
}
