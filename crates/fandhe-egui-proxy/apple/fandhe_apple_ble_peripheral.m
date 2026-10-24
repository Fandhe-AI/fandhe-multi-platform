// Phase 11 の拡充ループ（親 #164）型サンプル「BLE の周辺機器役」: macOS（Issue #778）と
// iOS（b-ios、Issue #779）の共用実装（`fandhe_apple_bluetooth_le.m` と同じ iOS/macOS 共用方針）。
// Foundation と CoreBluetooth だけを使い AppKit / UIKit は使わない。
//
// `crates/fandhe-egui-proxy/src/device/apple_ble_peripheral.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS / macOS の feature `device` 有効時に `cc` でコンパイルし、CoreBluetooth
// をリンクする。
//
// 契約:
//   int32_t fandhe_apple_ble_peripheral_start(uint64_t session, const char *service_uuid,
//       const char *characteristic_uuid, const char *local_name, const char *value)
//     引数の文字列は同期にコピーし、戻った後は参照しない。依頼を受けたら 0 を返し、処理は専用
//     シリアルキューで非同期に進む。引数が NULL なら 1 を返す。進行中の公開があれば完了通知なしで
//     破棄して置き換える（古い session の事象は Rust 側が照合して捨てる）。
//   int32_t fandhe_apple_ble_peripheral_stop(void)
//     専用キューへ同期ディスパッチして停止と解放を行う。完了コールバックは呼ばない。
//     0 = 停止した / 1 = 動いていなかった。呼び出し元は Rust 側のロックを握らないこと。
//   fandhe_rust_ble_peripheral_event(session, code)
//     Rust 側が公開する `extern "C"` 関数。code: 0 公開中（開始のコールバック成功）/ 1 読み取り 1 件 /
//     2 Bluetooth オフ / 3 権限拒否 / 4 非対応 / 5 失敗 / 6 中断（iOS の背景遷移、b-ios が発火）。
//     写像の SSOT は Rust 側 `fandhe_device::ble_peripheral` の純関数。終了系のコード（2〜6）は
//     1 つの公開につき高々 1 回（`FandheBlePeripheralFinish` が唯一の発火点）で、通知の前に
//     資源の解放を済ませる。
//
// スレッド: 専用のシリアルキュー（`dispatch_queue_create`）だけで動かし、main queue は使わない
// （main 側が取るロックとのデッドロックを避ける、`device-bridge-contracts.md` 11 節）。
// CBPeripheralManager もこのキューで作る。状態（g_*）への読み書きはすべてこのキュー上に閉じる。
//
// OS を正にする: 許可・電源状態を事前に問い合わせず、state の通知と `addService` /
// `startAdvertising` のコールバックの結果を写す。「公開中」(code 0) は
// `peripheralManagerDidStartAdvertising:error:` が成功したときにだけ送る。公開後に state が
// poweredOn を外れたら解放してから理由別のコードを送り、自動では再開しない。
//
// ログ方針: 中心役の識別子・読み取り要求の中身・OS のエラーコードを NSLog 等へ出さない
// （security.md A09）。

#import <Foundation/Foundation.h>
#import <CoreBluetooth/CoreBluetooth.h>
#import <TargetConditionals.h>

extern void fandhe_rust_ble_peripheral_event(uint64_t session, int32_t code);

enum {
    kFandheBlePeripheralAdvertising = 0,
    kFandheBlePeripheralRead = 1,
    kFandheBlePeripheralBluetoothOff = 2,
    kFandheBlePeripheralPermissionDenied = 3,
    kFandheBlePeripheralUnsupported = 4,
    kFandheBlePeripheralFailed = 5,
    kFandheBlePeripheralInterrupted = 6, // iOS の背景遷移（b-ios が使う）
};

@interface FandheBlePeripheralDelegate : NSObject <CBPeripheralManagerDelegate>
@property(nonatomic) uint64_t session;
@property(nonatomic, strong) CBUUID *serviceUUID;
@property(nonatomic, strong) CBUUID *characteristicUUID;
@property(nonatomic, copy) NSString *localName;
@property(nonatomic, strong) NSData *value;
@property(nonatomic) BOOL serviceRequested; // addService を呼んだ後は「登録以後」
@end

static dispatch_queue_t g_queue;
static CBPeripheralManager *g_manager = nil;
static FandheBlePeripheralDelegate *g_delegate = nil;

static dispatch_queue_t FandheBlePeripheralQueue(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      g_queue = dispatch_queue_create("com.fandhe.ble-peripheral", DISPATCH_QUEUE_SERIAL);
    });
    return g_queue;
}

// 公開を止めて資源を解放する（専用キュー上でのみ呼ぶ。コールバックは出さない）。
// 開始・登録に失敗した経路も含む全終了経路がここを通る（契約表 9）。
static void FandheBlePeripheralTeardown(void) {
    if (g_manager != nil) {
        if (g_manager.state == CBManagerStatePoweredOn) {
            [g_manager stopAdvertising];
            [g_manager removeAllServices];
        }
        g_manager.delegate = nil;
    }
    g_manager = nil;
    g_delegate = nil;
}

// 終了コードの唯一の発火点（専用キュー上でのみ呼ぶ）。現在の公開でなければ何もしない。
// 解放を済ませてから Rust へ通知する。
static void FandheBlePeripheralFinish(uint64_t session, int32_t code) {
    if (g_delegate == nil || g_delegate.session != session) {
        return;
    }
    FandheBlePeripheralTeardown();
    fandhe_rust_ble_peripheral_event(session, code);
}

// state が poweredOn でないときの理由コード。
static int32_t FandheBlePeripheralCodeForState(CBManagerState state) {
    switch (state) {
    case CBManagerStatePoweredOff:
        return kFandheBlePeripheralBluetoothOff;
    case CBManagerStateUnauthorized:
        return kFandheBlePeripheralPermissionDenied;
    case CBManagerStateUnsupported:
        return kFandheBlePeripheralUnsupported;
    default:
        // resetting / unknown（登録以後に届いたもの）は失敗として確定する。
        return kFandheBlePeripheralFailed;
    }
}

@implementation FandheBlePeripheralDelegate

- (void)peripheralManagerDidUpdateState:(CBPeripheralManager *)peripheral {
    // g_manager は init 戻り後に代入されるため、マネージャー同一性では判定しない
    // （init 中に届く初回状態を落とさない）。現在の公開かは delegate で判定する。
    if (g_delegate != self) {
        return;
    }
    if (peripheral.state == CBManagerStatePoweredOn) {
        if (self.serviceRequested) {
            return;
        }
        self.serviceRequested = YES;
        CBMutableCharacteristic *characteristic =
            [[CBMutableCharacteristic alloc] initWithType:self.characteristicUUID
                                               properties:CBCharacteristicPropertyRead
                                                    value:nil
                                              permissions:CBAttributePermissionsReadable];
        CBMutableService *service = [[CBMutableService alloc] initWithType:self.serviceUUID primary:YES];
        service.characteristics = @[ characteristic ];
        [peripheral addService:service];
        return;
    }
    if (self.serviceRequested) {
        // 登録以後に poweredOn を外れた: 解放して理由別に確定する（自動では再開しない）。
        FandheBlePeripheralFinish(self.session, FandheBlePeripheralCodeForState(peripheral.state));
        return;
    }
    switch (peripheral.state) {
    case CBManagerStatePoweredOff:
    case CBManagerStateUnauthorized:
    case CBManagerStateUnsupported:
        FandheBlePeripheralFinish(self.session, FandheBlePeripheralCodeForState(peripheral.state));
        break;
    default:
        // unknown / resetting は次の状態通知を待つ。
        break;
    }
}

- (void)peripheralManager:(CBPeripheralManager *)peripheral
            didAddService:(CBService *)service
                    error:(NSError *)error {
    if (g_delegate != self) {
        return;
    }
    if (error != nil) {
        FandheBlePeripheralFinish(self.session, kFandheBlePeripheralFailed);
        return;
    }
    // アドバタイズに載せられるのはローカル名とサービス UUID の 2 キーだけ。
    [peripheral startAdvertising:@{
        CBAdvertisementDataLocalNameKey : self.localName,
        CBAdvertisementDataServiceUUIDsKey : @[ self.serviceUUID ],
    }];
}

- (void)peripheralManagerDidStartAdvertising:(CBPeripheralManager *)peripheral error:(NSError *)error {
    if (g_delegate != self) {
        return;
    }
    if (error != nil) {
        FandheBlePeripheralFinish(self.session, kFandheBlePeripheralFailed);
        return;
    }
    // 「公開中」はここでしか送らない。
    fandhe_rust_ble_peripheral_event(self.session, kFandheBlePeripheralAdvertising);
}

- (void)peripheralManager:(CBPeripheralManager *)peripheral didReceiveReadRequest:(CBATTRequest *)request {
    // 各要求にちょうど 1 回応答する。
    BOOL current = (g_delegate == self);
    if (!current || ![request.characteristic.UUID isEqual:self.characteristicUUID]) {
        [peripheral respondToRequest:request withResult:CBATTErrorAttributeNotFound];
        return;
    }
    NSUInteger offset = request.offset;
    if (offset > self.value.length) {
        [peripheral respondToRequest:request withResult:CBATTErrorInvalidOffset];
        return;
    }
    request.value = [self.value subdataWithRange:NSMakeRange(offset, self.value.length - offset)];
    [peripheral respondToRequest:request withResult:CBATTErrorSuccess];
    if (offset == 0) {
        // 先頭からの読み取りだけを 1 件と数える（長い値の続き読みで二重に数えない）。
        fandhe_rust_ble_peripheral_event(self.session, kFandheBlePeripheralRead);
    }
}

@end

#if TARGET_OS_IPHONE
// b-ios（Issue #779）がここに背景遷移（UIApplicationDidEnterBackgroundNotification）を埋める。
// 購読したら専用キューで FandheBlePeripheralFinish(session, kFandheBlePeripheralInterrupted) を
// 呼んで止める。背景モードは宣言しないため、背景へ移るとアドバタイズが無効になる（OS の仕様）。
// (a) では空の受け口だけを置く。
#endif

int32_t fandhe_apple_ble_peripheral_start(uint64_t session, const char *service_uuid,
                                          const char *characteristic_uuid, const char *local_name,
                                          const char *value) {
    if (service_uuid == NULL || characteristic_uuid == NULL || local_name == NULL || value == NULL) {
        return 1;
    }
    // 戻る前に NSString / NSData へコピーし終える。
    NSString *serviceString = [NSString stringWithUTF8String:service_uuid];
    NSString *characteristicString = [NSString stringWithUTF8String:characteristic_uuid];
    NSString *name = [NSString stringWithUTF8String:local_name];
    NSString *valueString = [NSString stringWithUTF8String:value];
    if (serviceString == nil || characteristicString == nil || name == nil || valueString == nil) {
        return 1;
    }
    dispatch_async(FandheBlePeripheralQueue(), ^{
      @autoreleasepool {
          // 進行中の公開は置き換える（コールバックなし）。
          FandheBlePeripheralTeardown();

          FandheBlePeripheralDelegate *delegate = [[FandheBlePeripheralDelegate alloc] init];
          delegate.session = session;
          delegate.serviceUUID = [CBUUID UUIDWithString:serviceString];
          delegate.characteristicUUID = [CBUUID UUIDWithString:characteristicString];
          delegate.localName = name;
          delegate.value = [valueString dataUsingEncoding:NSUTF8StringEncoding];
          g_delegate = delegate;
          // 電源オフ時の OS 標準アラートは出さない（状態は画面の表示で伝える）。
          g_manager = [[CBPeripheralManager alloc]
              initWithDelegate:delegate
                         queue:FandheBlePeripheralQueue()
                       options:@{CBPeripheralManagerOptionShowPowerAlertKey : @NO}];
      }
    });
    return 0;
}

int32_t fandhe_apple_ble_peripheral_stop(void) {
    __block int32_t result = 1;
    dispatch_sync(FandheBlePeripheralQueue(), ^{
      @autoreleasepool {
          if (g_delegate != nil) {
              result = 0;
          }
          FandheBlePeripheralTeardown();
      }
    });
    return result;
}
