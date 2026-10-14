// Phase 11 の拡充ループ（親 #1118）型サンプル「予定の追加」: macOS 実装（Issue #1224）+
// iOS 実装（Issue #1225、`EKEventEditViewController`）。iOS / macOS 共用ファイル
// （`fandhe_apple_share.m` と同じ分け方。`#if TARGET_OS_IPHONE` が iOS 側）。
//
// `crates/fandhe-egui-proxy/src/device/apple_calendar.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos` が feature `device` 有効時に `cc` でコンパイルし、
// EventKit をリンクする。iOS は同じく `build.rs` の iOS 分岐が EventKit / EventKitUI を
// リンクする。
//
// 契約（pull 型。`fandhe_apple_biometric.m` と同型、coding-rust.md の FFI 例外規定）:
//   fandhe_apple_calendar_add_event(uint64_t request_id, const char *title,
//                                   const int32_t *start, const int32_t *end) -> int32_t
//     戻り値: 0 = 依頼を受理（結果は非同期に fandhe_rust_calendar_outcome で届く）、
//     1 = 受理できない（NULL / 不正な UTF-8）。title は NUL 終端 UTF-8、start / end は
//     長さ 5 の配列（年・月・日・時・分、端末のローカル時刻）。いずれも return する前に
//     値コピーするため、呼び出し元は return 後に解放してよい（所有権は移らない）。
//   fandhe_rust_calendar_outcome(uint64_t request_id, int32_t code)
//     Rust 側が公開する関数。code は device/calendar.rs::outcome_from_code の表が SSOT
//     （0 保存 / 1 権限拒否 / 2 macOS 14 未満 / 3 保存失敗 / 4 権限要求エラー /
//      5 既定カレンダーなし / 6 日時変換失敗 / 7 利用者が編集画面を閉じた（iOS のみ、
//      キャンセル。失敗ではない） / 8 編集画面を出せなかった（iOS のみ））。
//      各依頼につきちょうど 1 回だけ呼ぶ。
//
// iOS: 権限は求めない。`EKEventEditViewController` はプロセス外で描画され許可を要さない
// （Apple「Accessing Calendar using EventKit and EventKitUI」）ため usage description も
// 足さない。保存は OS が行い、アプリは編集画面で変えた内容を読めない。delegate の
// Saved のみ code 0。Canceled / Deleted は 7、未知の action は 3（成功を作らない）。
// 提示は main queue（UIKit 必須）。実機確認は #1214（人手）。
//
// 権限は write-only（`requestWriteOnlyAccessToEventsWithCompletion:`、macOS 14+）だけを
// 求める。予定の読み取り API は使わない。旧 `requestAccessToEntityType:` は非推奨で
// 旧キー NSCalendarsUsageDescription も要るため使わない（macOS 14 未満は code 2）。
// write-only で `defaultCalendarForNewEvents` を使う根拠は Apple の WWDC23
// "Discover Calendar and EventKit"（write-only 権限は既定カレンダーへの追加用）。
// 実機での動作確認は #1214（人手）。
//
// スレッド: EKEventStore への操作はすべて専用 serial queue に閉じる（契約表 11）。
// 権限ダイアログは OS が出す（こちらから UI を出さない）ため main queue は不要。
// 各ブロックは @autoreleasepool で包む（契約表 5）。
//
// 件名・日時・NSError の内容はログへ出さない（NSLog 不使用、security.md A09）。

#import <Foundation/Foundation.h>
#import <EventKit/EventKit.h>
#import <TargetConditionals.h>
#if TARGET_OS_IPHONE
#import <EventKitUI/EventKitUI.h>
#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"
#endif

extern void fandhe_rust_calendar_outcome(uint64_t request_id, int32_t code);

static NSDate *FandheDate(const int32_t c[5]) {
    NSDateComponents *dc = [[NSDateComponents alloc] init];
    dc.year = c[0];
    dc.month = c[1];
    dc.day = c[2];
    dc.hour = c[3];
    dc.minute = c[4];
    // 利用者の暦設定（仏暦等）に依らず、Rust 側 parse_local の検査と同じグレゴリオ暦で解釈する。
    // タイムゾーンは端末ローカル（NSCalendar の既定）のまま。
    NSCalendar *gregorian = [[NSCalendar alloc] initWithCalendarIdentifier:NSCalendarIdentifierGregorian];
    gregorian.timeZone = [NSTimeZone localTimeZone];
    return [gregorian dateFromComponents:dc];
}

#if TARGET_OS_IPHONE

// 提示中の delegate（`editViewDelegate` は weak）を生かす強参照。main queue からのみ読み書きする。
static id _Nullable g_retainedDelegate = nil;

@interface FandheCalendarEditDelegate : NSObject <EKEventEditViewDelegate>
@property(nonatomic) uint64_t requestId;
@property(nonatomic) BOOL done;
@end

@implementation FandheCalendarEditDelegate
- (void)eventEditViewController:(EKEventEditViewController *)controller
          didCompleteWithAction:(EKEventEditViewAction)action {
    if (self.done) {
        return;
    }
    self.done = YES;
    int32_t code;
    switch (action) {
    case EKEventEditViewActionSaved:
        code = 0;
        break;
    case EKEventEditViewActionCanceled:
    case EKEventEditViewActionDeleted: // 新規予定では出ないが、届いてもキャンセル扱い
        code = 7;
        break;
    default:
        code = 3;
        break;
    }
    uint64_t requestId = self.requestId;
    g_retainedDelegate = nil;
    [controller dismissViewControllerAnimated:YES completion:nil];
    fandhe_rust_calendar_outcome(requestId, code);
}
@end

int32_t fandhe_apple_calendar_add_event(uint64_t request_id, const char *title,
                                        const int32_t *start, const int32_t *end) {
    if (title == NULL || start == NULL || end == NULL) {
        return 1;
    }
    NSString *nsTitle = [NSString stringWithUTF8String:title];
    if (nsTitle == nil) {
        return 1;
    }
    NSDate *nsStart = FandheDate(start);
    NSDate *nsEnd = FandheDate(end);

    dispatch_async(dispatch_get_main_queue(), ^{
      @autoreleasepool {
        if (nsStart == nil || nsEnd == nil) {
            fandhe_rust_calendar_outcome(request_id, 6);
            return;
        }
        if (@available(iOS 17.0, *)) {
            UIViewController *top = FandheTopmostViewController();
            if (top == nil || top.view.window == nil) {
                fandhe_rust_calendar_outcome(request_id, 8);
                return;
            }
            EKEventStore *store = [[EKEventStore alloc] init];
            EKEvent *event = [EKEvent eventWithEventStore:store];
            event.title = nsTitle;
            event.startDate = nsStart;
            event.endDate = nsEnd;
            // calendar は設定しない（権限なしでは既定を引けないため、編集画面が選ぶ）。
            FandheCalendarEditDelegate *delegate = [[FandheCalendarEditDelegate alloc] init];
            delegate.requestId = request_id;
            EKEventEditViewController *vc = [[EKEventEditViewController alloc] init];
            vc.eventStore = store;
            vc.event = event;
            vc.editViewDelegate = delegate;
            // スワイプで閉じると delegate が呼ばれないため、閉じる操作をボタンに限る。
            vc.modalInPresentation = YES;
            g_retainedDelegate = delegate;
            [top presentViewController:vc animated:YES completion:nil];
            // 最前面が別 VC の dismiss 中などで提示が無視されると delegate も呼ばれず、
            // 画面が「追加中」のまま残る。提示されなかったときは提示失敗として返す。
            if (top.presentedViewController != vc) {
                g_retainedDelegate = nil;
                delegate.done = YES;
                fandhe_rust_calendar_outcome(request_id, 8);
            }
        } else {
            fandhe_rust_calendar_outcome(request_id, 2);
        }
      }
    });
    return 0;
}

#else

static dispatch_queue_t FandheCalendarQueue(void) {
    static dispatch_queue_t q;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      q = dispatch_queue_create("com.fandhe.egui-proxy.calendar", DISPATCH_QUEUE_SERIAL);
    });
    return q;
}

// serial queue 上で保存して code を返す。store は許可後に新規生成したものを呼び出し側が渡す
// （認可前に作った store は許可後も calendar の見え方が古いままになり得るため、使い回さない）。
static int32_t FandheSaveEvent(EKEventStore *store, NSString *title, NSDate *start, NSDate *end) {
    EKCalendar *calendar = [store defaultCalendarForNewEvents];
    if (calendar == nil) {
        return 5;
    }
    EKEvent *event = [EKEvent eventWithEventStore:store];
    event.title = title;
    event.startDate = start;
    event.endDate = end;
    event.calendar = calendar;
    NSError *error = nil;
    return [store saveEvent:event span:EKSpanThisEvent commit:YES error:&error] ? 0 : 3;
}

int32_t fandhe_apple_calendar_add_event(uint64_t request_id, const char *title,
                                        const int32_t *start, const int32_t *end) {
    if (title == NULL || start == NULL || end == NULL) {
        return 1;
    }
    NSString *nsTitle = [NSString stringWithUTF8String:title];
    if (nsTitle == nil) {
        return 1;
    }
    NSDate *nsStart = FandheDate(start);
    NSDate *nsEnd = FandheDate(end);

    dispatch_async(FandheCalendarQueue(), ^{
      @autoreleasepool {
        if (nsStart == nil || nsEnd == nil) {
            fandhe_rust_calendar_outcome(request_id, 6);
            return;
        }
        if (@available(macOS 14.0, *)) {
            // 許可要求専用の store。保存には使わず、許可後に新しい store を作る。
            EKEventStore *requestStore = [[EKEventStore alloc] init];
            [requestStore
                requestWriteOnlyAccessToEventsWithCompletion:^(BOOL granted, NSError *_Nullable error) {
                  // completion は任意スレッドから来る。serial queue へ戻して処理する。
                  // requestStore を completion が済むまで保持する（ARC で先に解放されないよう捕捉）。
                  (void)requestStore;
                  dispatch_async(FandheCalendarQueue(), ^{
                    @autoreleasepool {
                      int32_t code;
                      if (error != nil) {
                          code = 4;
                      } else if (!granted) {
                          code = 1;
                      } else {
                          code = FandheSaveEvent([[EKEventStore alloc] init], nsTitle, nsStart, nsEnd);
                      }
                      fandhe_rust_calendar_outcome(request_id, code);
                    }
                  });
                }];
        } else {
            fandhe_rust_calendar_outcome(request_id, 2);
        }
      }
    });
    return 0;
}

#endif
