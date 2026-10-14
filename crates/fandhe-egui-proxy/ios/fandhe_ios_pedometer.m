// Phase 11 の拡充ループ（親 #1116）型サンプル「歩数」: iOS 実装（Issue #1248）。
//
// `crates/fandhe-egui-proxy/src/device/ios_pedometer.rs` の `IosPedometer::read_steps` から
// C ABI 経由で呼ばれる。`build.rs` の `build_ios()` が feature `device` 有効時にのみ
// コンパイルする。CoreMotion は `fandhe_ios_motion.m` のためにリンク済み。
// 権限宣言は `platforms/apple/FandheIosApp/Resources/Info.plist` の `NSMotionUsageDescription`。
//
// 契約:
//   fandhe_ios_pedometer_read(uint64_t timeout_ms, int64_t *out_steps) -> int32_t
//     ワーカースレッドから呼ぶブロッキング関数（main thread では呼ばない）。今日 0 時から
//     現在までの累積歩数を `queryPedometerData` で 1 回取得し、semaphore で `timeout_ms`
//     まで待つ。期限は Rust 側 `READ_TIMEOUT` の引数だけが決め、ここに固定値は持たない。
//   戻り値: 0 = OK（`*out_steps` を書く。0 歩も正当な成功）/ 1 = 非対応 /
//           2 = 権限拒否 / 3 = 未取得（期限切れ・データ無し）/ 4 = その他の失敗。
//           0 以外のとき `*out_steps` は書き換えない。
//
// メモリ安全: handler は期限切れの後に遅れて呼ばれうる。handler が書くのは block が
// 保持する `__block` 変数だけで、呼び出し元スタックの `out_steps` には触れない
// （`out_steps` へは semaphore を受けた後に本関数が書く）。
// 取り消し: query に cancel API は無い。関数を抜けて `CMPedometer` が ARC で解放されることを
// 期限切れ時の取り消しとして扱う。登録が残る listener は無い。
// ログ: 歩数・NSError は出力しない。

#import <CoreMotion/CoreMotion.h>
#import <Foundation/Foundation.h>

int32_t fandhe_ios_pedometer_read(uint64_t timeout_ms, int64_t *out_steps) {
    @autoreleasepool {
        if (![CMPedometer isStepCountingAvailable]) {
            return 1;
        }
        CMAuthorizationStatus auth = [CMPedometer authorizationStatus];
        if (auth == CMAuthorizationStatusDenied || auth == CMAuthorizationStatusRestricted) {
            return 2;
        }

        // 初回 (NotDetermined) はこの query で OS が許可ダイアログを出す。
        __block int32_t code = 4;
        __block int64_t steps = 0;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        // 待機中に ARC が最後の使用点で解放すると進行中のクエリが取り消されハンドラが走らない。
        // objc_precise_lifetime で スコープ末尾まで寿命を延ばす。
        __attribute__((objc_precise_lifetime)) CMPedometer *pedometer = [[CMPedometer alloc] init];
        NSDate *now = [NSDate date];
        NSDate *from = [[NSCalendar currentCalendar] startOfDayForDate:now];
        [pedometer queryPedometerDataFromDate:from
                                       toDate:now
                                  withHandler:^(CMPedometerData *data, NSError *error) {
            if (error != nil) {
                // 初回ダイアログで拒否すると CMErrorNotAuthorized（code=105）が届く。
                // 取得前の authorizationStatus 検査ではこの経路を拾えない。
                BOOL cmDomain = [error.domain isEqualToString:CMErrorDomain];
                if (cmDomain && (error.code == CMErrorNotAuthorized ||
                                 error.code == CMErrorMotionActivityNotAuthorized)) {
                    code = 2;
                } else if (error.code == CMErrorMotionActivityNotAvailable) {
                    code = 1;
                } else {
                    code = 4;
                }
            } else if (data == nil) {
                code = 3;
            } else {
                steps = data.numberOfSteps.longLongValue;
                code = 0;
            }
            dispatch_semaphore_signal(sem);
        }];

        dispatch_time_t deadline =
            dispatch_time(DISPATCH_TIME_NOW, (int64_t)(timeout_ms * NSEC_PER_MSEC));
        if (dispatch_semaphore_wait(sem, deadline) != 0) {
            return 3;
        }
        if (code == 0) {
            *out_steps = steps;
        }
        return code;
    }
}
