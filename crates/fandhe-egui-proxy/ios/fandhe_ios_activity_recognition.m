// 行動認識（歩行・乗車等）: iOS 実装（Issue #614、親 #124）。CoreMotion の
// `CMMotionActivityManager` で現在の行動を購読し、最新値を static に保持する（pull 型）。
//
// `src/device/ios_activity_recognition.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（CoreMotion は motion 用にリンク済み）。
//
// 契約:
//   fandhe_ios_activity_recognition_start() -> int32_t
//       0 = 成功（冪等）、1 = 非対応（`isActivityAvailable` が NO。シミュレータはここ。何も確保しない）、
//       2 = 権限拒否（authorizationStatus が Denied / Restricted）、3 = 失敗（main 以外からの呼び出しを含む）
//   fandhe_ios_activity_recognition_read(uint32_t *flags, int32_t *confidence, uint64_t *seq) -> int32_t
//       0 = 値なし、1 = 値あり（out へ書く）、2 = 権限拒否、3 = 失敗、4 = 許可ダイアログの応答待ち。
//       NULL は 3。戻り値が 1 以外のとき out は書き換えない。
//       flags: bit0 stationary / bit1 walking / bit2 running / bit3 automotive / bit4 cycling
//       （`unknown` は渡さない。全部 0 が「判定なし」）。confidence: 0 Low / 1 Medium / 2 High。
//   fandhe_ios_activity_recognition_stop()  冪等。購読を解除し、保持値と失敗を捨てる。
//   値・失敗を確定した直後に Rust の `fandhe_rust_activity_recognition_changed()` を呼ぶ（再描画要求）。
//   背景遷移で `..._backgrounded()`、前景復帰で `..._resumed()` を呼ぶ（ドライバの待機を止める・引き直す）。
//
// 流れ: start で manager を作り live 更新を専用 serial queue で受ける（開始直後に現在の行動が 1 件届く）。
// live 更新の handler は NSError を受けないため、拒否コード（CMErrorNotAuthorized と
// CMErrorMotionActivityNotAuthorized の 2 種）は短い範囲の probe query の NSError から拾う。
// 結果の配列（過去の履歴）は捨てる。read のたびに authorizationStatus を確かめる。
// nil・判定なし（全フラグ NO）は「静止」にせず、nil は保持値を捨てて値なしにする。
//
// 世代: 購読ごとに manager を作り直し、handler が捕捉した epoch をロック内で現在値と照合して旧購読分を捨てる。
// seq はプロセス内で単調増加させ、stop・再開始・復帰で巻き戻さない。
// stop は背景中なら Rust 側の背景フラグも下ろす（resumed）。背景・前景の observer と専用 queue も解除・解放し、再開始で登録し直す。
// 背景遷移: 保持値を捨て、購読を解除する（suspend 中は届かないため）。復帰で張り直す。
// 許可ダイアログ表示中のアプリは Inactive で Background にならない。observer は
// DidEnterBackground / WillEnterForeground だけを見るため、許可の応答との到着順に依存しない。
// 期限（開始から 5 秒）はドライバの `FirstReadingDeadline` だけが持ち、ここでは時間を測らない。
//
// スレッド: start は main 限定。stop は main 以外なら状態をすぐ捨て、`stopActivityUpdates` を
// `dispatch_async` で main へ送る（`dispatch_sync` は使わない。`device-bridge-contracts.md`「11」）。
// 状態は os_unfair_lock で守り、ロック内で Rust・CoreMotion を呼ばない。
//
// 守る範囲: 嘘の値を出さない・二重購読しない・停止と背景で解除する・旧購読の通知を捨てる。
// 守らない範囲: OS 間の分類の一致、判定の遅れ、過去の履歴、背景中の受信、複数プロセス。
// 既知の天井: 許可後 5 秒以内に最初の 1 件が届くか、probe query の挙動は実機未確認（#603）。
// ログは出さない（種別・確度・NSError を出さない。security.md A09）。

#import <CoreMotion/CoreMotion.h>
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <os/lock.h>

// Rust 側 `#[no_mangle]`（引数なし・panic しない）。
extern void fandhe_rust_activity_recognition_changed(void);
extern void fandhe_rust_activity_recognition_backgrounded(void);
extern void fandhe_rust_activity_recognition_resumed(void);

static os_unfair_lock gLock = OS_UNFAIR_LOCK_INIT;
static BOOL gActive = NO;     // 購読を望んでいるか
static BOOL gBackground = NO; // 背景中は購読を張らず、通知も捨てる
static uint64_t gEpoch = 0;   // 開始・停止・背景遷移・復帰で進める世代
static BOOL gHasValue = NO;
static uint32_t gFlags = 0;
static int32_t gConfidence = 0;
static uint64_t gSeq = 0;     // 単調増加。巻き戻さない
static int32_t gFailure = 0;  // 0 = なし、2 = 権限拒否、3 = 失敗
static CMMotionActivityManager *gManager = nil;
static NSOperationQueue *gQueue = nil;
static id gBackgroundToken = nil;
static id gForegroundToken = nil;

// probe query で見る範囲（秒）。結果の配列は使わず NSError だけを見る。
static const NSTimeInterval kProbeSeconds = 5.0;

static BOOL IsDenied(CMAuthorizationStatus s) {
    return s == CMAuthorizationStatusDenied || s == CMAuthorizationStatusRestricted;
}

// ロックの外で呼ぶ。manager の live 更新と probe query を張る。epoch は張った時点の世代。
static void Subscribe(CMMotionActivityManager *m, NSOperationQueue *queue, uint64_t epoch) {
    [m startActivityUpdatesToQueue:queue
                       withHandler:^(CMMotionActivity *_Nullable a) {
        os_unfair_lock_lock(&gLock);
        if (!gActive || gBackground || epoch != gEpoch) { // 旧購読・背景中の通知は捨てる
            os_unfair_lock_unlock(&gLock);
            return;
        }
        if (a == nil) {
            gHasValue = NO; // 直前値を返し続けない
        } else {
            uint32_t f = 0;
            if (a.stationary) f |= 1u << 0;
            if (a.walking) f |= 1u << 1;
            if (a.running) f |= 1u << 2;
            if (a.automotive) f |= 1u << 3;
            if (a.cycling) f |= 1u << 4;
            gFlags = f;
            gConfidence = (int32_t)a.confidence;
            gSeq += 1;
            gHasValue = YES;
            gFailure = 0; // 実サンプルが届いた以上、遅れて届いた probe の失敗は無効
        }
        os_unfair_lock_unlock(&gLock);
        fandhe_rust_activity_recognition_changed();
    }];
    NSDate *now = [NSDate date];
    [m queryActivityStartingFromDate:[now dateByAddingTimeInterval:-kProbeSeconds]
                              toDate:now
                             toQueue:queue
                         withHandler:^(NSArray<CMMotionActivity *> *_Nullable acts, NSError *_Nullable error) {
        if (error == nil) {
            return;
        }
        // 許可ダイアログの応答待ち（NotDetermined）の間は probe の失敗を確定しない。
        // ライブ更新が許可後に届く前に機能を失敗させないため（read も 4 を優先して返す）
        if ([CMMotionActivityManager authorizationStatus] == CMAuthorizationStatusNotDetermined) {
            return;
        }
        os_unfair_lock_lock(&gLock);
        if (!gActive || gBackground || epoch != gEpoch) {
            os_unfair_lock_unlock(&gLock);
            return;
        }
        if (gHasValue) { // live 値が既にある。遅れて届いた probe の失敗では動作中の購読を落とさない
            os_unfair_lock_unlock(&gLock);
            return;
        }
        BOOL cmDomain = [error.domain isEqualToString:CMErrorDomain];
        if (cmDomain && (error.code == CMErrorNotAuthorized ||
                         error.code == CMErrorMotionActivityNotAuthorized)) {
            gFailure = 2;
        } else {
            gFailure = 3; // 未知のエラーも成功へ畳まない
        }
        gHasValue = NO;
        os_unfair_lock_unlock(&gLock);
        fandhe_rust_activity_recognition_changed();
    }];
}

int32_t fandhe_ios_activity_recognition_start(void) {
    if (![NSThread isMainThread]) {
        return 3;
    }
    @autoreleasepool {
        if (![CMMotionActivityManager isActivityAvailable]) {
            return 1;
        }
        if (IsDenied([CMMotionActivityManager authorizationStatus])) {
            return 2;
        }
        os_unfair_lock_lock(&gLock);
        if (gActive) {
            os_unfair_lock_unlock(&gLock);
            return 0;
        }
        if (gBackgroundToken == nil) { // stop で解除済み。再開始で登録し直す
            gQueue = [[NSOperationQueue alloc] init];
            gQueue.maxConcurrentOperationCount = 1;
            gBackgroundToken = [NSNotificationCenter.defaultCenter
                addObserverForName:UIApplicationDidEnterBackgroundNotification
                            object:nil
                             queue:nil
                        usingBlock:^(NSNotification *__unused note) {
                @autoreleasepool {
                    os_unfair_lock_lock(&gLock);
                    if (!gActive) { // 解除と到着が入れ替わった通知は捨てる
                        os_unfair_lock_unlock(&gLock);
                        return;
                    }
                    gBackground = YES;
                    gHasValue = NO;
                    gEpoch += 1;
                    CMMotionActivityManager *old = gManager;
                    gManager = nil;
                    os_unfair_lock_unlock(&gLock);
                    [old stopActivityUpdates];
                    fandhe_rust_activity_recognition_backgrounded();
                }
            }];
            gForegroundToken = [NSNotificationCenter.defaultCenter
                addObserverForName:UIApplicationWillEnterForegroundNotification
                            object:nil
                             queue:nil
                        usingBlock:^(NSNotification *__unused note) {
                @autoreleasepool {
                    os_unfair_lock_lock(&gLock);
                    if (!gActive) {
                        os_unfair_lock_unlock(&gLock);
                        return;
                    }
                    gBackground = NO;
                    gHasValue = NO;
                    gFailure = 0;
                    gEpoch += 1;
                    uint64_t epoch = gEpoch;
                    NSOperationQueue *q = gQueue;
                    CMMotionActivityManager *m = [[CMMotionActivityManager alloc] init];
                    gManager = m;
                    os_unfair_lock_unlock(&gLock);
                    if (q != nil) {
                        Subscribe(m, q, epoch);
                    }
                    fandhe_rust_activity_recognition_resumed();
                }
            }];
        }
        gEpoch += 1;
        uint64_t epoch = gEpoch;
        gHasValue = NO;
        gFailure = 0;
        gBackground = UIApplication.sharedApplication.applicationState == UIApplicationStateBackground;
        gActive = YES;
        CMMotionActivityManager *m = nil;
        NSOperationQueue *q = gQueue;
        if (!gBackground) { // 背景中は張らず、復帰時に張る
            m = [[CMMotionActivityManager alloc] init];
            gManager = m;
        }
        os_unfair_lock_unlock(&gLock);
        if (m != nil) {
            Subscribe(m, q, epoch);
        } else {
            // 背景中の開始: 購読を省くので、ドライバの初回値期限を止める（復帰の resumed で引き直す）
            fandhe_rust_activity_recognition_backgrounded();
        }
    }
    return 0;
}

int32_t fandhe_ios_activity_recognition_read(uint32_t *flags, int32_t *confidence, uint64_t *seq) {
    if (flags == NULL || confidence == NULL || seq == NULL) {
        return 3;
    }
    @autoreleasepool {
        CMAuthorizationStatus auth = [CMMotionActivityManager authorizationStatus];
        int32_t rc = 0;
        os_unfair_lock_lock(&gLock);
        if (!gActive) {
            rc = 0;
        } else if (IsDenied(auth)) {
            rc = 2;
        } else if (auth == CMAuthorizationStatusNotDetermined) {
            rc = 4; // 応答待ちは過去の失敗より優先する
        } else if (gFailure != 0) {
            rc = gFailure;
        } else if (gBackground || !gHasValue) {
            rc = 0;
        } else {
            *flags = gFlags;
            *confidence = gConfidence;
            *seq = gSeq;
            rc = 1;
        }
        os_unfair_lock_unlock(&gLock);
        return rc;
    }
}

void fandhe_ios_activity_recognition_stop(void) {
    @autoreleasepool {
        os_unfair_lock_lock(&gLock);
        // 背景中に止めると前景 observer も解除され resumed が届かない。Rust 側の背景フラグを
        // 下ろさないと、次の start が永久に遮断される
        BOOL wasBackground = gActive && gBackground;
        gBackground = NO;
        CMMotionActivityManager *old = gManager;
        gManager = nil;
        gEpoch += 1; // 旧購読の通知はここから照合で捨てられる
        gActive = NO;
        gHasValue = NO;
        gFailure = 0;
        // ライフサイクル通知の購読と専用 queue を解放する。再開始で登録し直す
        id bg = gBackgroundToken;
        id fg = gForegroundToken;
        gBackgroundToken = nil;
        gForegroundToken = nil;
        gQueue = nil;
        os_unfair_lock_unlock(&gLock);
        if (bg != nil) {
            [NSNotificationCenter.defaultCenter removeObserver:bg];
        }
        if (fg != nil) {
            [NSNotificationCenter.defaultCenter removeObserver:fg];
        }
        if (wasBackground) {
            fandhe_rust_activity_recognition_resumed();
        }
        if (old == nil) {
            return;
        }
        if ([NSThread isMainThread]) {
            [old stopActivityUpdates];
        } else {
            dispatch_async(dispatch_get_main_queue(), ^{
                [old stopActivityUpdates];
            });
        }
    }
}
