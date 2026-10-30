// Phase 11 の拡充ループ（親 #147）型サンプル「スマートホーム操作」: iOS 実装（Issue #1080）。
// HomeKit の `HMHomeManager` で、登録済みの家の数と（家の名前, 機器の名前）を OS の返す順に先頭 20 件だけ読む。
// 機器の操作・状態の読み取り・家の追加はしない。家・機器の名前と NSError はログに出さない。
//
// `crates/fandhe-egui-proxy/src/device/ios_smart_home.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルし、HomeKit をリンクする。
// 権限宣言は `platforms/apple/FandheIosApp/Resources/Info.plist` の `NSHomeKitUsageDescription`
// （無いと初回の HomeKit 利用でクラッシュする）と、同ターゲットの entitlement `com.apple.developer.homekit`。
//
// 契約:
//   fandhe_ios_smart_home_list(timeout_ms, buf, cap, out_len, out_count, out_home_count, out_overflowed)
//     ワーカースレッドから呼ぶブロッキング関数（main thread では呼ばない）。
//     期限の起点は本関数の入口で、許可ダイアログの応答待ちと読み込み待ちで同じ期限を共用する。
//     戻り値: 0 = OK（家 0 件・機器 0 件も正当な成功）/ 1 = 非対応（返さない）/ 2 = 権限拒否 /
//             3 = 期限切れ / 4 = その他の失敗（引数不正・多重起動）。
//             0 以外のとき buf と out_* は書き換えない。
//     成功時、buf へ「home\0name\0」の組を機器の数だけ詰める。1 要素は 1023 バイトまで
//     （UTF-8 の文字境界で切る）。制御文字は除く。名前の無い項目は空文字列。
//     out_home_count は登録済みの家の数、21 件目以降の機器がある、または領域が足りなければ
//     *out_overflowed = 1。
//
// 嘘の成功の回避:
//   - 生成直後の空の `homes` は読まない（`homeManagerDidUpdateHomes:` の初回通知が読み込み完了）。
//   - `authorizationStatus` に `authorized` が立つまで 0 件を返さない。許可の前に届いた通知は
//     読み込み完了として数えず、その後に再通知が来なければ 3（期限切れ）になる。
//   - 家 0 件と機器 0 件は out_home_count で区別する。
//
// スレッド: HMHomeManager・delegate・本ファイルの静的状態は main queue だけが触る。delegate が
// どの queue で呼ばれるかはヘッダに明記が無いため、delegate 側で必ず `dispatch_async(main)` に載せ直す。
// ワーカーは semaphore で待つだけ。期限切れは要求の atomic な取消旗を立てて即座に戻り、main queue を待たない
// （main が詰まっていても期限内に戻り、g_in_flight も戻る時点で解除される）。main 側の遅延した登録・評価は
// 実行時に取消旗を見て、立っていれば何もしない。
// ワーカーはロックを持たず（旗は atomic のみ）、main は本関数を待たないのでデッドロックしない。
// 期限後に届いた結果は ctx にだけ書かれ、呼び出し元の buf・out_* へは触れない。
// manager と delegate はプロセスで 1 つ、意図して解放しない（初回の要求で生成）。
//
// 再描画: delegate は共有状態を書かず semaphore で同じワーカーへ戻るだけ。画面へ結果を入れるのは
// Rust 側の CompletionSender で、そこで再描画が起こされる。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。HMHome・HMAccessory は main block の外へ出さず、
// 持ち出すのは NSString の写しだけ。

#import <Foundation/Foundation.h>
#import <HomeKit/HomeKit.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>

#define FANDHE_ACC_MAX 20
#define FANDHE_ACC_FIELD_MAX 1023

static atomic_bool g_in_flight = false;

// 制御文字（NUL を含む）を除いた文字列。nil は空文字列。
static NSString *FandheCleanField(NSString *s) {
    if (s == nil) {
        return @"";
    }
    return [[s componentsSeparatedByCharactersInSet:[NSCharacterSet controlCharacterSet]]
        componentsJoinedByString:@""];
}

// 要求 1 件ぶんの文脈。main queue 上でだけ書き、signal の後にワーカーが読む。
@interface FandheSmartHomeWaiter : NSObject
@property(nonatomic, strong) dispatch_semaphore_t sem;
@property(nonatomic) int32_t code;
@property(nonatomic) NSUInteger homeCount;
@property(nonatomic) BOOL overflow;
// 期限切れでワーカーが戻ったら立てる（atomic）。main 側の遅延処理は実行時に見て、立っていれば何もしない。
@property(atomic) BOOL cancelled;
@property(nonatomic, strong) NSMutableArray<NSArray<NSString *> *> *rows;
@end
@implementation FandheSmartHomeWaiter
@end

@interface FandheSmartHomeDelegate : NSObject <HMHomeManagerDelegate>
@end

static HMHomeManager *g_manager = nil;
static FandheSmartHomeDelegate *g_delegate = nil;
static BOOL g_loaded = NO;
static FandheSmartHomeWaiter *g_waiter = nil;

// main queue 上で呼ぶ。待ち中の要求があり、許可と読み込みが確定していれば結果を入れて起こす。
static void FandheEvaluate(void) {
    FandheSmartHomeWaiter *ctx = g_waiter;
    if (ctx == nil || g_manager == nil) {
        return;
    }
    if (ctx.cancelled) {
        g_waiter = nil;
        return;
    }
    HMHomeManagerAuthorizationStatus status = g_manager.authorizationStatus;
    if (status & HMHomeManagerAuthorizationStatusAuthorized) {
        if (!g_loaded) {
            return;
        }
        NSArray<HMHome *> *homes = g_manager.homes;
        ctx.homeCount = homes.count;
        BOOL overflow = NO;
        for (HMHome *home in homes) {
            for (HMAccessory *acc in home.accessories) {
                if (ctx.rows.count == FANDHE_ACC_MAX) {
                    overflow = YES;
                    break;
                }
                [ctx.rows addObject:@[ FandheCleanField(home.name), FandheCleanField(acc.name) ]];
            }
            if (overflow) {
                break;
            }
        }
        ctx.overflow = overflow;
        ctx.code = 0;
    } else if ((status & HMHomeManagerAuthorizationStatusDetermined) != 0) {
        // 確定済みで authorized が無い（restricted を含む）は拒否。
        ctx.code = 2;
    } else {
        return;
    }
    g_waiter = nil;
    dispatch_semaphore_signal(ctx.sem);
}

@implementation FandheSmartHomeDelegate
- (void)homeManagerDidUpdateHomes:(HMHomeManager *)manager {
    dispatch_async(dispatch_get_main_queue(), ^{
        // 許可が出る前に届いた空の読み込みは完了扱いにしない。
        if (g_manager.authorizationStatus & HMHomeManagerAuthorizationStatusAuthorized) {
            g_loaded = YES;
        }
        FandheEvaluate();
    });
}
- (void)homeManager:(HMHomeManager *)manager
    didUpdateAuthorizationStatus:(HMHomeManagerAuthorizationStatus)status {
    dispatch_async(dispatch_get_main_queue(), ^{
        FandheEvaluate();
    });
}
@end

int32_t fandhe_ios_smart_home_list(uint64_t timeout_ms, uint8_t *buf, size_t cap,
                                   size_t *out_len, uint32_t *out_count,
                                   uint32_t *out_home_count, int32_t *out_overflowed) {
    if (buf == NULL || out_len == NULL || out_count == NULL || out_home_count == NULL ||
        out_overflowed == NULL) {
        return 4;
    }
    if (atomic_exchange(&g_in_flight, true)) {
        return 4;
    }
    int32_t result = 4;
    @autoreleasepool {
        dispatch_time_t deadline =
            dispatch_time(DISPATCH_TIME_NOW, (int64_t)(timeout_ms * NSEC_PER_MSEC));
        FandheSmartHomeWaiter *ctx = [FandheSmartHomeWaiter new];
        ctx.sem = dispatch_semaphore_create(0);
        ctx.code = 4;
        ctx.rows = [NSMutableArray array];

        dispatch_async(dispatch_get_main_queue(), ^{
            if (g_manager == nil) {
                g_delegate = [FandheSmartHomeDelegate new];
                g_manager = [HMHomeManager new];
                g_manager.delegate = g_delegate;
            }
            if (ctx.cancelled) {
                return;
            }
            g_waiter = ctx;
            FandheEvaluate();
        });

        if (dispatch_semaphore_wait(ctx.sem, deadline) != 0) {
            // 期限切れ。main queue を待たず、要求を取消済みにして戻る。
            // 登録前なら main 側が登録を飛ばし、登録後なら次の評価か次の要求の登録で外れる。
            ctx.cancelled = YES;
            result = 3;
        } else if (ctx.code != 0) {
            result = ctx.code;
        } else {
            size_t pos = 0;
            uint32_t written = 0;
            BOOL overflow = ctx.overflow;
            for (NSArray<NSString *> *row in ctx.rows) {
                // 1 組（2 要素 × (最大 1023 バイト + NUL)）が収まらなければ止める。
                if (cap - pos < 2 * (FANDHE_ACC_FIELD_MAX + 1)) {
                    overflow = YES;
                    break;
                }
                for (NSString *field in row) {
                    NSUInteger used = 0;
                    [field getBytes:buf + pos
                          maxLength:FANDHE_ACC_FIELD_MAX
                         usedLength:&used
                           encoding:NSUTF8StringEncoding
                            options:0
                              range:NSMakeRange(0, field.length)
                     remainingRange:NULL];
                    pos += used;
                    buf[pos++] = 0;
                }
                written++;
            }
            *out_len = pos;
            *out_count = written;
            *out_home_count = (uint32_t)ctx.homeCount;
            *out_overflowed = overflow ? 1 : 0;
            result = 0;
        }
    }
    atomic_store(&g_in_flight, false);
    return result;
}
