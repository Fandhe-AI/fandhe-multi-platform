// Phase 11 の拡充ループ（親 #130）型サンプル「Bluetooth Classic」: macOS 実装（Issue #659）。
// IOBluetooth の `IOBluetoothDevice.pairedDevices`（ペアリング済み一覧）と
// `IOBluetoothDeviceInquiry`（周辺の機器の検索）を使う。接続・ペアリング操作・電源の ON 操作はしない。
// 機器のアドレス文字列は同一機器の判定キーとして Rust 側へ渡すだけで、画面にもログにも出さない。
//
// `crates/fandhe-egui-proxy/src/device/macos_bluetooth_classic.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし IOBluetooth をリンクする。
//
// 契約（検索は pull 型。`fandhe_apple_lan_discovery.m` と同型）:
//   int32_t fandhe_macos_bt_classic_paired(FandheBtClassicListRaw *out)
//     UI（main）スレッドから同期で呼ぶ。戻り値は完了コード（0 成功 / 1 許可なし / 2 Bluetooth オフ /
//     3 非対応 / 4 失敗）。0 以外では out は無効。count は 0..=50（超過分は書かない）。
//     `pairedDevices` が nil のときは 0 件の成功に畳まず 4 を返す。電波強度は検索中でないため
//     常に 127（未取得）。key は機器の addressString で空の機器は書かない。名前は NUL 終端 UTF-8
//     （最大 255 バイト、文字境界で切る）。取れなければ空文字。
//   int32_t fandhe_macos_bt_classic_start(uint64_t request_id, uint64_t duration_ms)
//     常に 0（受理）を返し、処理は main queue で非同期に進む。進行中の検索があれば完了コールバック
//     なしで破棄して置き換える（古い request_id の結果は Rust 側が照合して捨てる）。
//     duration_ms は Rust 側 `DISCOVERY_WINDOW_MS`（SSOT）で、窓の起点はこの呼び出しの入口
//     （単調時計で絶対期限を確定し、main queue の遅延も窓に含める。期限切れなら検索せず 4 で確定）。
//   void fandhe_macos_bt_classic_stop(void)
//     終了時専用。main thread 上なら直接、それ以外なら main queue へ同期ディスパッチして検索を止め
//     解放する。完了コールバックは呼ばない。呼び出し元は Rust 側のロックを握らないこと
//     （`device-bridge-contracts.md` 11 節。main 上なら dispatch_sync しないのでデッドロックしない）。
//   fandhe_rust_bt_classic_found(request_id, key, name_or_null, cod, rssi, paired)
//   fandhe_rust_bt_classic_finished(request_id, code)
//     Rust 側が公開する `extern "C"` 関数。found は機器の通知ごとに main queue から呼ぶ（rssi は
//     127 で未取得）。finished は 1 要求につき高々 1 回で、次のうち早いものを 1 回だけ採る:
//       タイマー（窓の終わり）→ 0 / complete で正常 → 0 / complete で error・aborted（OS の間引きを
//       含む）→ 4。確定したら必ず inquiry を stop し delegate を外してから finished を呼ぶ。
//       stop の後に届く complete は無視する。コードの意味づけは Rust 側
//       `fandhe_device::bluetooth_classic::decode_code` に一本化する。
//
// 電源と権限の判定（一覧と検索で共通）:
//   `IOBluetoothHostController.defaultController` が nil → 3、`powerState` が ON でない → 2、
//   `CBManager.authorization` が Denied / Restricted → 1（NotDetermined は先へ進める）。
//   【推測】IOBluetooth への TCC 適用は公式に記載がなく、CoreBluetooth の許可状態で代用している。
//   Mac 目視（#650）で確かめる。
//
// スレッド: IOBluetooth の delegate は開始したスレッドの run loop に届くため、検索は main queue に
// 閉じる（BLE の専用キューとは違う）。セッション状態はすべて main に閉じる。
// FFI 規約: 全体を `@autoreleasepool` で囲む。名前・アドレスを NSLog 等へ出力しない（security.md A09）。

#pragma clang diagnostic ignored "-Wdeprecated-declarations"
#import <Foundation/Foundation.h>
#import <CoreBluetooth/CoreBluetooth.h>
#import <IOBluetooth/IOBluetooth.h>
#include <stdint.h>
#include <string.h>
#include <time.h>

#define FANDHE_BT_MAX 50
#define FANDHE_BT_NAME_BYTES 256
#define FANDHE_BT_KEY_BYTES 32

typedef struct {
    char name[FANDHE_BT_NAME_BYTES];
    uint32_t cod;
    int32_t rssi;
    char key[FANDHE_BT_KEY_BYTES];
    int32_t paired;
} FandheBtClassicDeviceRaw;

typedef struct {
    int32_t count;
    FandheBtClassicDeviceRaw entries[FANDHE_BT_MAX];
} FandheBtClassicListRaw;

extern void fandhe_rust_bt_classic_found(uint64_t request_id, const char *key, const char *name_or_null,
                                         uint32_t cod, int32_t rssi, int32_t paired);
extern void fandhe_rust_bt_classic_finished(uint64_t request_id, int32_t code);

// 完了コード（Rust 側 `decode_code` と対）。
enum {
    kFandheBtOk = 0,
    kFandheBtPermissionDenied = 1,
    kFandheBtDisabled = 2,
    kFandheBtUnsupported = 3,
    kFandheBtFailed = 4,
};

// 電源と権限を判定する。0 なら先へ進める。
static int32_t FandheBtPrecheck(void) {
    IOBluetoothHostController *controller = [IOBluetoothHostController defaultController];
    if (controller == nil) {
        return kFandheBtUnsupported;
    }
    if (controller.powerState != kBluetoothHCIPowerStateON) {
        return kFandheBtDisabled;
    }
    CBManagerAuthorization auth = [CBManager authorization];
    if (auth == CBManagerAuthorizationDenied || auth == CBManagerAuthorizationRestricted) {
        return kFandheBtPermissionDenied;
    }
    return kFandheBtOk;
}

// NSString を UTF-8 で dst（capacity バイト）へ書く。文字境界で切り、取れなければ空文字。
static void FandheBtCopy(NSString *s, char *dst, NSUInteger capacity) {
    dst[0] = '\0';
    if (s == nil) {
        return;
    }
    NSUInteger used = 0;
    if ([s getBytes:dst
             maxLength:capacity - 1
            usedLength:&used
              encoding:NSUTF8StringEncoding
               options:0
                 range:NSMakeRange(0, s.length)
        remainingRange:NULL]) {
        dst[used] = '\0';
    } else {
        dst[0] = '\0';
    }
}

int32_t fandhe_macos_bt_classic_paired(FandheBtClassicListRaw *out) {
    if (out == NULL) {
        return kFandheBtFailed;
    }
    @autoreleasepool {
        out->count = 0;
        int32_t pre = FandheBtPrecheck();
        if (pre != kFandheBtOk) {
            return pre;
        }
        NSArray *devices = [IOBluetoothDevice pairedDevices];
        if (devices == nil) {
            // 0 件の成功に畳まない（nil を返す条件は #650 で確かめる）。
            return kFandheBtFailed;
        }
        for (id obj in devices) {
            if (out->count >= FANDHE_BT_MAX) {
                break;
            }
            if (![obj isKindOfClass:[IOBluetoothDevice class]]) {
                continue;
            }
            IOBluetoothDevice *device = (IOBluetoothDevice *)obj;
            NSString *key = device.addressString;
            if (key.length == 0) {
                continue;
            }
            FandheBtClassicDeviceRaw *e = &out->entries[out->count];
            FandheBtCopy(key, e->key, FANDHE_BT_KEY_BYTES);
            if (e->key[0] == '\0') {
                continue;
            }
            FandheBtCopy(device.name, e->name, FANDHE_BT_NAME_BYTES);
            e->cod = (uint32_t)device.classOfDevice;
            e->rssi = 127;
            e->paired = 1;
            out->count++;
        }
    }
    return kFandheBtOk;
}

// ---- 検索 ----

@interface FandheBtSession : NSObject
@property(nonatomic) uint64_t requestId;
@property(nonatomic, strong) IOBluetoothDeviceInquiry *inquiry;
@property(nonatomic) BOOL finished;
@end

@implementation FandheBtSession
@end

static FandheBtSession *g_bt_session = nil;

// セッションの inquiry を止めて参照を外す（main 上でのみ呼ぶ。コールバックは出さない）。
static void FandheBtTeardown(FandheBtSession *session) {
    if (session == nil) {
        return;
    }
    session.finished = YES;
    IOBluetoothDeviceInquiry *inquiry = session.inquiry;
    session.inquiry = nil;
    if (inquiry != nil) {
        [inquiry stop];
        inquiry.delegate = nil;
    }
    if (g_bt_session == session) {
        g_bt_session = nil;
    }
}

// 完了を 1 回だけ確定する（main 上でのみ呼ぶ）。stop と delegate 解除を済ませてから通知する。
static void FandheBtFinish(FandheBtSession *session, int32_t code) {
    if (session == nil || session.finished) {
        return;
    }
    uint64_t request_id = session.requestId;
    FandheBtTeardown(session);
    fandhe_rust_bt_classic_finished(request_id, code);
}

@interface FandheBtDelegate : NSObject <IOBluetoothDeviceInquiryDelegate>
@property(nonatomic, weak) FandheBtSession *session;
@end

@implementation FandheBtDelegate

- (void)notify:(IOBluetoothDevice *)device {
    FandheBtSession *session = self.session;
    if (session == nil || session.finished) {
        return;
    }
    NSString *key = device.addressString;
    if (key.length == 0) {
        return;
    }
    char keyBuf[FANDHE_BT_KEY_BYTES];
    FandheBtCopy(key, keyBuf, sizeof(keyBuf));
    if (keyBuf[0] == '\0') {
        return;
    }
    char nameBuf[FANDHE_BT_NAME_BYTES];
    FandheBtCopy(device.name, nameBuf, sizeof(nameBuf));
    fandhe_rust_bt_classic_found(session.requestId, keyBuf, nameBuf[0] == '\0' ? NULL : nameBuf,
                                 (uint32_t)device.classOfDevice, (int32_t)device.rawRSSI,
                                 device.isPaired ? 1 : 0);
}

- (void)deviceInquiryDeviceFound:(IOBluetoothDeviceInquiry *)sender device:(IOBluetoothDevice *)device {
    @autoreleasepool {
        [self notify:device];
    }
}

- (void)deviceInquiryDeviceNameUpdated:(IOBluetoothDeviceInquiry *)sender
                                device:(IOBluetoothDevice *)device
                      devicesRemaining:(uint32_t)devicesRemaining {
    @autoreleasepool {
        [self notify:device];
    }
}

- (void)deviceInquiryComplete:(IOBluetoothDeviceInquiry *)sender error:(IOReturn)error aborted:(BOOL)aborted {
    @autoreleasepool {
        FandheBtSession *session = self.session;
        if (session == nil || session.finished) {
            return;
        }
        FandheBtFinish(session, (error != kIOReturnSuccess || aborted) ? kFandheBtFailed : kFandheBtOk);
    }
}

@end

// delegate はセッションが弱参照するため、セッションと同じ寿命で強参照を持つ。
static FandheBtDelegate *g_bt_delegate = nil;

// 単調時計（dispatch_time の DISPATCH_TIME_NOW と同じ uptime 基準）の現在値 [ns]。
static uint64_t FandheBtNowNs(void) {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

int32_t fandhe_macos_bt_classic_start(uint64_t request_id, uint64_t duration_ms) {
    // 期限は呼び出しの入口で確定する（main queue の遅延・事前判定の時間も窓に含める。
    // device-bridge-contracts.md 3 節）。
    const uint64_t deadline_ns = FandheBtNowNs() + duration_ms * NSEC_PER_MSEC;
    dispatch_async(dispatch_get_main_queue(), ^{
      @autoreleasepool {
          // 進行中の検索は完了コールバックなしで置き換える。
          FandheBtTeardown(g_bt_session);
          g_bt_delegate = nil;

          // main queue の遅延で既に期限を過ぎていれば、検索せず失敗で確定する
          // （遅れて集めた機器を成功として返さない）。
          if (FandheBtNowNs() >= deadline_ns) {
              fandhe_rust_bt_classic_finished(request_id, kFandheBtFailed);
              return;
          }
          int32_t pre = FandheBtPrecheck();
          if (pre != kFandheBtOk) {
              fandhe_rust_bt_classic_finished(request_id, pre);
              return;
          }
          if (FandheBtNowNs() >= deadline_ns) {
              fandhe_rust_bt_classic_finished(request_id, kFandheBtFailed);
              return;
          }
          uint64_t remaining_ns = deadline_ns - FandheBtNowNs();
          uint64_t remaining_ms = remaining_ns / NSEC_PER_MSEC;
          FandheBtSession *session = [[FandheBtSession alloc] init];
          session.requestId = request_id;
          FandheBtDelegate *delegate = [[FandheBtDelegate alloc] init];
          delegate.session = session;
          IOBluetoothDeviceInquiry *inquiry = [IOBluetoothDeviceInquiry inquiryWithDelegate:delegate];
          if (inquiry == nil) {
              fandhe_rust_bt_classic_finished(request_id, kFandheBtFailed);
              return;
          }
          uint64_t seconds = (remaining_ms + 999) / 1000;
          if (seconds == 0) {
              seconds = 1;
          }
          inquiry.inquiryLength = (uint8_t)(seconds > 255 ? 255 : seconds);
          inquiry.updateNewDeviceNames = YES;
          session.inquiry = inquiry;
          g_bt_session = session;
          g_bt_delegate = delegate;
          if ([inquiry start] != kIOReturnSuccess) {
              FandheBtFinish(session, kFandheBtFailed);
              return;
          }
          // 窓を閉じるタイマー。入口で確定した期限までの残り時間で設定する（単調時計）。
          // complete が先に来た場合は finished で無視される。
          uint64_t now_ns = FandheBtNowNs();
          uint64_t timer_ns = deadline_ns > now_ns ? deadline_ns - now_ns : 0;
          dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)timer_ns),
                         dispatch_get_main_queue(), ^{
                           @autoreleasepool {
                               FandheBtFinish(session, kFandheBtOk);
                           }
                         });
      }
    });
    return 0;
}

static void FandheBtStopOnMain(void) {
    @autoreleasepool {
        FandheBtTeardown(g_bt_session);
        g_bt_delegate = nil;
    }
}

void fandhe_macos_bt_classic_stop(void) {
    if ([NSThread isMainThread]) {
        FandheBtStopOnMain();
    } else {
        dispatch_sync(dispatch_get_main_queue(), ^{
          FandheBtStopOnMain();
        });
    }
}
