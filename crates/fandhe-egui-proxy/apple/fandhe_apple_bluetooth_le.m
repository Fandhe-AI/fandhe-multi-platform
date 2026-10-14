// Phase 11 の拡充ループ（親 #1059）型サンプル「BLE スキャン」: macOS（Issue #1060）と
// iOS（Issue #1061）の共用実装（`fandhe_apple_biometric.m` と同じ iOS/macOS 共用方針）。
// Foundation と CoreBluetooth だけを使い AppKit / UIKit は使わない。
//
// `crates/fandhe-egui-proxy/src/device/apple_bluetooth_le.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS / macOS の feature `device` 有効時に `cc` でコンパイルし、CoreBluetooth
// をリンクする。
//
// 契約（pull 型。`fandhe_apple_speech_recognition.m` と同型）:
//   int32_t fandhe_apple_ble_start(uint64_t request_id, uint64_t duration_ms)
//     常に 0（受理）を返し、処理は専用シリアルキューで非同期に進む。進行中のセッションが
//     あれば完了コールバックなしで破棄して置き換える（古い request_id の結果は Rust 側が
//     照合して捨てる）。duration_ms は Rust 側 `SCAN_DURATION_MS`（SSOT）で、PoweredOn
//     になってからこの時間で `stopScan` して完了する（許可ダイアログの回答待ちには期限が
//     無い。終了で止まる）。
//   void fandhe_apple_ble_stop(void)
//     終了時専用。専用キューへ同期ディスパッチして `stopScan` と解放を行う。完了
//     コールバックは呼ばない。呼び出し元は Rust 側のロックを握らないこと。
//   fandhe_rust_ble_device(request_id, id, name_or_null, rssi)
//   fandhe_rust_ble_finished(request_id, code)
//     Rust 側が公開する `extern "C"` 関数。code: 0 成功 / 1 Bluetooth オフ /
//     2 権限拒否 / 3 非対応 / 4 内部エラー。コードの意味づけ（契約のエラー種別への
//     写像）は Rust 側 `device/bluetooth_le.rs` の純関数に一本化する。1 要求につき
//     finished は高々 1 回（`FandheBleFinish` が唯一の発火点）。rssi は取れないとき
//     127（番兵）を渡す。
//
// スレッド: 専用のシリアルキュー（`dispatch_queue_create`）だけで動かし、main queue は
// 使わない（main 側が取るロックとのデッドロックを避ける、`device-bridge-contracts.md`
// 11 節）。セッション状態（g_*）への読み書きはすべてこのキュー上に閉じる。
//
// 背景遷移（iOS）: 失敗として扱わない（オーナー判断 10/5、`fandhe-device` の
// `bluetooth_le` 契約）。監視も順序判定もせず、期限で通常どおり終了して届いた結果を成功で
// 返す。背景中は OS が広告を間引くため件数が減りうる（既知の制約）。背景モードは宣言しない。
//
// 失敗の扱い: スキャン途中で Bluetooth がオフになった・リセットされた場合も失敗として
// 終え、部分結果を成功にしない（`device-bridge-contracts.md` 6 節）。周辺機器の名前・識別子を
// NSLog 等へ出力しない（security.md A09）。

#import <Foundation/Foundation.h>
#import <CoreBluetooth/CoreBluetooth.h>

extern void fandhe_rust_ble_device(uint64_t request_id, const char *id, const char *name_or_null, int32_t rssi);
extern void fandhe_rust_ble_finished(uint64_t request_id, int32_t code);

@interface FandheBleDelegate : NSObject <CBCentralManagerDelegate>
@property(nonatomic) uint64_t requestId;
@property(nonatomic) uint64_t durationMs;
@property(nonatomic) BOOL scanStarted;
@end

static dispatch_queue_t g_ble_queue;
static CBCentralManager *g_ble_manager = nil;
static FandheBleDelegate *g_ble_delegate = nil;

static dispatch_queue_t FandheBleQueue(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      g_ble_queue = dispatch_queue_create("com.fandhe.ble-scan", DISPATCH_QUEUE_SERIAL);
    });
    return g_ble_queue;
}

// セッションを止めて破棄する（専用キュー上でのみ呼ぶ。コールバックは出さない）。
static void FandheBleTeardown(void) {
    if (g_ble_manager != nil) {
        if (g_ble_manager.isScanning) {
            [g_ble_manager stopScan];
        }
        g_ble_manager.delegate = nil;
    }
    g_ble_manager = nil;
    g_ble_delegate = nil;
}

// 完了の唯一の発火点（専用キュー上でのみ呼ぶ）。対象が現在のセッションでなければ何もしない。
static void FandheBleFinish(uint64_t request_id, int32_t code) {
    if (g_ble_delegate == nil || g_ble_delegate.requestId != request_id) {
        return;
    }
    FandheBleTeardown();
    fandhe_rust_ble_finished(request_id, code);
}

@implementation FandheBleDelegate

- (void)centralManagerDidUpdateState:(CBCentralManager *)central {
    // g_ble_manager は init 戻り後に代入されるため、マネージャー同一性では判定しない
    // （init 中に届く初回状態を落とさない）。現在のセッションかは delegate で判定する。
    if (g_ble_delegate != self) {
        return;
    }
    switch (central.state) {
    case CBManagerStatePoweredOn: {
        if (self.scanStarted) {
            return;
        }
        self.scanStarted = YES;
        // 重複通知は抑える（通知量を減らす。同一機器の RSSI は初回観測値になる）。
        [central scanForPeripheralsWithServices:nil
                                        options:@{CBCentralManagerScanOptionAllowDuplicatesKey : @NO}];
        uint64_t rid = self.requestId;
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(self.durationMs * NSEC_PER_MSEC)),
                       FandheBleQueue(), ^{
                         FandheBleFinish(rid, 0);
                       });
        break;
    }
    case CBManagerStatePoweredOff:
        // スキャン途中でも失敗にする（部分結果を成功にしない）。
        FandheBleFinish(self.requestId, 1);
        break;
    case CBManagerStateUnauthorized:
        FandheBleFinish(self.requestId, 2);
        break;
    case CBManagerStateUnsupported:
        FandheBleFinish(self.requestId, 3);
        break;
    case CBManagerStateResetting:
        // スキャン開始後のリセットでスキャンは止まる（PoweredOn に戻っても再開しない）。
        // 期限で成功扱いになる嘘の成功を避け、内部エラーとして確定する。開始前なら待つ。
        if (self.scanStarted) {
            FandheBleFinish(self.requestId, 4);
        }
        break;
    default:
        // Unknown は次の状態通知を待つ。
        break;
    }
}

- (void)centralManager:(CBCentralManager *)central
 didDiscoverPeripheral:(CBPeripheral *)peripheral
     advertisementData:(NSDictionary<NSString *, id> *)advertisementData
                  RSSI:(NSNumber *)RSSI {
    if (g_ble_delegate != self) {
        return;
    }
    NSString *name = advertisementData[CBAdvertisementDataLocalNameKey];
    if (![name isKindOfClass:[NSString class]] || name.length == 0) {
        name = peripheral.name;
    }
    const char *idC = peripheral.identifier.UUIDString.UTF8String;
    const char *nameC = (name.length > 0) ? name.UTF8String : NULL;
    int32_t rssi = (RSSI != nil) ? (int32_t)RSSI.intValue : 127;
    if (idC != NULL) {
        fandhe_rust_ble_device(self.requestId, idC, nameC, rssi);
    }
}

@end

int32_t fandhe_apple_ble_start(uint64_t request_id, uint64_t duration_ms) {
    dispatch_async(FandheBleQueue(), ^{
      @autoreleasepool {
          // 進行中のセッションは置き換える（コールバックなし）。
          FandheBleTeardown();

          if (@available(macOS 10.15, iOS 13.1, *)) {
              CBManagerAuthorization auth = CBCentralManager.authorization;
              if (auth == CBManagerAuthorizationDenied || auth == CBManagerAuthorizationRestricted) {
                  fandhe_rust_ble_finished(request_id, 2);
                  return;
              }
          }

          FandheBleDelegate *delegate = [[FandheBleDelegate alloc] init];
          delegate.requestId = request_id;
          delegate.durationMs = duration_ms;
          g_ble_delegate = delegate;
          // 電源オフ時の OS 標準アラートは出さない（状態は画面の失敗表示で伝える）。
          g_ble_manager = [[CBCentralManager alloc]
              initWithDelegate:delegate
                         queue:FandheBleQueue()
                       options:@{CBCentralManagerOptionShowPowerAlertKey : @NO}];
      }
    });
    return 0;
}

void fandhe_apple_ble_stop(void) {
    dispatch_sync(FandheBleQueue(), ^{
      @autoreleasepool {
          FandheBleTeardown();
      }
    });
}
