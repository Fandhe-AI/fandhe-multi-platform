// Phase 11 の拡充ループ（親 #149）型サンプル「ヘルスデータ読み書き」: iOS 実装（Issue #1090）。
// HealthKit で今日 0 時から現在までの歩数の合計を 1 回だけ読む。書き込み・他の型・履歴・背景配信はしない。
//
// `crates/fandhe-egui-proxy/src/device/ios_health_data.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルし、HealthKit をリンクする。
// 権限宣言は `platforms/apple/FandheIosApp/Resources/Info.plist` の `NSHealthShareUsageDescription`
// （無いと許可の要求でクラッシュする）と、同ターゲットの entitlement `com.apple.developer.healthkit`。
//
// 契約:
//   fandhe_ios_health_data_read(uint64_t timeout_ms, double *out_steps) -> int32_t
//     ワーカースレッドから呼ぶブロッキング関数（main thread では呼ばない）。
//     期限の起点は入口で、許可シートの待ちと集計の待ちで同じ期限（単調時計）を共用する。
//   戻り値: 0 = 歩数（`*out_steps` を書く。0 歩も正当）/ 1 = 記録が見えない（記録が無い、または
//           読み取りが許可されていない。OS は両者を区別させない）/ 2 = 非対応 / 3 = 期限切れ /
//           4 = 取り消し（許可シートの `HKErrorUserCanceled`）/ 5 = その他の失敗
//           （引数不正・多重起動・端末ロック中・entitlement 不足を含む）。
//           0 以外のとき `*out_steps` は書き換えない。値域の検査は Rust 側 `steps_from` に任せる。
//
// 嘘の成功の回避: `requestAuthorization` の `success` は許可の有無を表さないので、許可の判定には使わない。
// 拒否された型は「データ無し」に見える（`errorNoData`・`sumQuantity` が nil）ため 1 に写す。
// 0 歩・失敗には畳まない。コードは必ず `HKErrorDomain` を確かめてから見る。
//
// 多重起動: `g_in_flight` で 1 件に絞る。許可シートは取り消せないので、期限切れの後も completion が
// 届くまで `g_auth_pending` を残し、その間の次の要求は 5 で断る。
//
// メモリ安全: completion と handler は期限切れの後に遅れて呼ばれうる。書くのは block が保持する
// `__block` 変数だけで、呼び出し元スタックの `out_steps` には触れない（semaphore を受けた後に本関数が書く）。
// `HKHealthStore` はプロセスで 1 つ、意図して解放しない。登録が残る listener は無い。
// 再描画: OS コールバックは共有状態を書かず semaphore で同じワーカーへ戻るだけ。画面へ結果を入れるのは
// Rust 側の CompletionSender で、そこで再描画が起こされる。
// ログ: 歩数・NSError は出力しない。

#import <Foundation/Foundation.h>
#import <HealthKit/HealthKit.h>
#include <stdatomic.h>
#include <stdint.h>

static atomic_bool g_in_flight = false;
static atomic_bool g_auth_pending = false;

static HKHealthStore *FandheHealthStore(void) {
    static HKHealthStore *store;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        store = [[HKHealthStore alloc] init];
    });
    return store;
}

// 本体。入口の旗の管理は呼び出し側が行う。
static int32_t FandheReadSteps(uint64_t timeout_ms, double *out_steps) {
    if (![HKHealthStore isHealthDataAvailable]) {
        return 2;
    }
    // 乗算が溢れない上限（約 292 年）に丸める。
    if (timeout_ms > (uint64_t)(INT64_MAX / NSEC_PER_MSEC)) {
        timeout_ms = (uint64_t)(INT64_MAX / NSEC_PER_MSEC);
    }
    dispatch_time_t deadline =
        dispatch_time(DISPATCH_TIME_NOW, (int64_t)(timeout_ms * NSEC_PER_MSEC));
    HKHealthStore *store = FandheHealthStore();
    HKQuantityType *type = [HKQuantityType quantityTypeForIdentifier:HKQuantityTypeIdentifierStepCount];
    if (type == nil) {
        return 5;
    }

    // 許可。書き込みの型は空、読み取りは歩数だけ。
    __block int32_t authCode = 0;
    dispatch_semaphore_t authSem = dispatch_semaphore_create(0);
    atomic_store(&g_auth_pending, true);
    [store requestAuthorizationToShareTypes:[NSSet set]
                                  readTypes:[NSSet setWithObject:type]
                                 completion:^(BOOL success, NSError *error) {
        if (!success) {
            BOOL hkDomain = error != nil && [error.domain isEqualToString:HKErrorDomain];
            if (hkDomain && error.code == HKErrorUserCanceled) {
                authCode = 4;
            } else if (hkDomain && (error.code == HKErrorHealthDataUnavailable ||
                                    error.code == HKErrorHealthDataRestricted)) {
                authCode = 2;
            } else {
                authCode = 5;
            }
        }
        atomic_store(&g_auth_pending, false);
        dispatch_semaphore_signal(authSem);
    }];
    if (dispatch_semaphore_wait(authSem, deadline) != 0) {
        return 3;
    }
    if (authCode != 0) {
        return authCode;
    }

    // 集計。
    NSDate *now = [NSDate date];
    NSDate *from = [[NSCalendar currentCalendar] startOfDayForDate:now];
    NSPredicate *pred = [HKQuery predicateForSamplesWithStartDate:from
                                                          endDate:now
                                                          options:HKQueryOptionStrictStartDate];
    __block int32_t code = 5;
    __block double steps = 0;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    __attribute__((objc_precise_lifetime)) HKStatisticsQuery *query = [[HKStatisticsQuery alloc]
        initWithQuantityType:type
     quantitySamplePredicate:pred
                     options:HKStatisticsOptionCumulativeSum
           completionHandler:^(HKStatisticsQuery *__unused q, HKStatistics *result, NSError *error) {
        if (error != nil) {
            BOOL hkDomain = [error.domain isEqualToString:HKErrorDomain];
            if (hkDomain && error.code == HKErrorNoData) {
                code = 1;
            } else if (hkDomain && (error.code == HKErrorHealthDataUnavailable ||
                                    error.code == HKErrorHealthDataRestricted)) {
                code = 2;
            } else {
                // 端末ロック中（HKErrorDatabaseInaccessible）を含む。
                code = 5;
            }
        } else if (result.sumQuantity == nil) {
            code = 1;
        } else {
            steps = [result.sumQuantity doubleValueForUnit:[HKUnit countUnit]];
            code = 0;
        }
        dispatch_semaphore_signal(sem);
    }];
    [store executeQuery:query];
    if (dispatch_semaphore_wait(sem, deadline) != 0) {
        [store stopQuery:query];
        return 3;
    }
    if (code == 0) {
        *out_steps = steps;
    }
    return code;
}

int32_t fandhe_ios_health_data_read(uint64_t timeout_ms, double *out_steps) {
    @autoreleasepool {
        if (out_steps == NULL) {
            return 5;
        }
        bool expected = false;
        if (!atomic_compare_exchange_strong(&g_in_flight, &expected, true)) {
            return 5;
        }
        int32_t code = 5;
        if (atomic_load(&g_auth_pending)) {
            code = 5;
        } else {
            code = FandheReadSteps(timeout_ms, out_steps);
        }
        atomic_store(&g_in_flight, false);
        return code;
    }
}
