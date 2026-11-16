// Phase 11 の拡充ループ（親 #887）型サンプル「位置情報」: iOS / macOS 共通の実装
// （Issue #889）。CoreLocation（`CLLocationManager`）は iOS と macOS で同一 API のため、
// 1 ファイルを両 OS で共用する（`ios/` 配下の他ファイルと違い `apple/` に置く理由）。
//
// `crates/fandhe-egui-proxy/src/device/apple_location.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS・macOS 両ターゲット（feature `device` 有効時）で `cc` コンパイルし、
// CoreLocation フレームワークをリンクする。
//
// 契約（push 型。`fandhe_ios_tts.m` の delegate → Rust 通知と同型、
// coding-rust.md の FFI 例外規定）:
//   fandhe_apple_location_start(void) -> 戻り値無し。冪等（複数回呼んでも副作用は
//     初回のみ `CLLocationManager` を生成する。2 回目以降は現在の
//     `authorizationStatus` を再評価して結果を再 push する。再試行ボタン押下時に
//     Rust 側が [`fandhe_rust_location_...`] を再受信するための経路）。
//   fandhe_rust_location_fix(double lat_deg, double lon_deg, double accuracy_m)
//     -> Rust 側（`apple_location.rs`）が公開する `extern "C"` 関数。有効な 1 測位を
//        push する。
//   fandhe_rust_location_failure(int32_t code) -> 同じく Rust 側が公開する関数。
//        1 = denied（権限拒否）、2 = disabled（位置情報サービス自体が無効）、
//        3 = failed（その他の失敗）。
//
// 未取得（Waiting）は Rust 側の `SharedLocation` だけが表現し、ObjC 側は「有効な
// 座標」か「失敗コード」のみを push する（sentinel 値を両側に持たない、実装計画
// §3.1 参照）。`horizontalAccuracy < 0`（CoreLocation の契約上、無効なサンプル）は
// push しない。
//
// スレッド: `CLLocationManager` は生成したスレッドの run loop を要求するため、
// `fandhe_apple_location_start` はメインスレッド以外から呼ばれた場合
// `dispatch_async` でメインキューへ載せ替える。delegate コールバックは常にメイン
// スレッドで届き、そこから直接 Rust 側 `extern "C"` 関数を呼ぶ（`SharedLocation` は
// `Mutex` のためどのスレッドからでも安全）。
//
// 座標をログへ出力しない（security.md A09）。
//
// ponytail: `stopUpdatingLocation` の明示呼び出しは行わない（`fandhe_ios_motion.m`
// と同じ単純化）。iOS は suspend で更新が止まる。明示停止・電力最適化が要件化
// したら `stop` エクスポートを追加する。
// ponytail: `desiredAccuracy` は既定（`kCLLocationAccuracyBest`）のまま固定する。
// 精度・電力のトレードオフ調整が要件化したら設定可能にする。

#import <CoreLocation/CoreLocation.h>
#import <TargetConditionals.h>

// Rust 側（`apple_location.rs`）が公開する `extern "C"` 関数。
extern void fandhe_rust_location_fix(double lat_deg, double lon_deg, double accuracy_m);
extern void fandhe_rust_location_failure(int32_t code);

// プロセス内で 1 個だけ保持する manager/delegate（`fandhe_ios_tts.m` の
// `g_synthesizer` と同じ dispatch_once パターン）。
static CLLocationManager *g_location_manager = nil;
static dispatch_once_t g_location_manager_once;

@interface FandheLocationDelegate : NSObject <CLLocationManagerDelegate>
@end

// 現在の `authorizationStatus` を評価し、権限・サービス状態に応じて
// 測位開始 or 失敗 push のいずれかを行う（`start` 初回・再試行の両方から呼ばれる
// 共通処理）。
static void FandheEvaluateAuthorization(CLLocationManager *manager) {
    CLAuthorizationStatus status = manager.authorizationStatus;
    switch (status) {
        case kCLAuthorizationStatusNotDetermined:
#if TARGET_OS_OSX
            // ネイティブ macOS では `requestWhenInUseAuthorization` はプロンプトを
            // 表示しない（iOS 専用の挙動）。macOS 用の `requestAlwaysAuthorization`
            // を呼ぶ（Info.plist の `NSLocationUsageDescription` と対応、Cursor
            // Bugbot 指摘）。
            [manager requestAlwaysAuthorization];
#else
            [manager requestWhenInUseAuthorization];
#endif
            // 結果は `locationManagerDidChangeAuthorization:` で非同期に届く。
            // ここでは push しない（Waiting のまま）。
            break;
        case kCLAuthorizationStatusRestricted:
            fandhe_rust_location_failure(1);
            break;
        case kCLAuthorizationStatusDenied:
            if ([CLLocationManager locationServicesEnabled]) {
                fandhe_rust_location_failure(1);
            } else {
                fandhe_rust_location_failure(2);
            }
            break;
        case kCLAuthorizationStatusAuthorizedAlways:
#if !TARGET_OS_OSX
        // `kCLAuthorizationStatusAuthorizedWhenInUse` は iOS のみに存在し macOS の
        // CoreLocation ヘッダでは `API_UNAVAILABLE(macos)`（使用中のみ権限は
        // `AuthorizedAlways` に統合されている）。
        case kCLAuthorizationStatusAuthorizedWhenInUse:
#endif
            [manager startUpdatingLocation];
            // 既に authorized 済みで `startUpdatingLocation` が動いている manager
            // では、この呼び出しは新規イベントを発火しない（cursor[bot] 指摘:
            // 「再試行」で `fandhe_apple_location_start` → 本関数が再入しても、
            // CoreLocation は状態変化が無ければ delegate を呼び直さないため
            // waiting のまま止まる）。キャッシュ済みの最新座標があれば、ここで
            // 即座に再 push して解消する（`delegate` の `didUpdateLocations` を
            // 待たずに直近の既知値を返す、`manager.location` は CoreLocation が
            // 保持するキャッシュ）。
            if (manager.location != nil && manager.location.horizontalAccuracy >= 0) {
                fandhe_rust_location_fix(manager.location.coordinate.latitude,
                                          manager.location.coordinate.longitude,
                                          manager.location.horizontalAccuracy);
            }
            break;
        default:
            fandhe_rust_location_failure(3);
            break;
    }
}

@implementation FandheLocationDelegate

- (void)locationManagerDidChangeAuthorization:(CLLocationManager *)manager {
    FandheEvaluateAuthorization(manager);
}

- (void)locationManager:(CLLocationManager *)manager didUpdateLocations:(NSArray<CLLocation *> *)locations {
    CLLocation *location = locations.lastObject;
    if (location == nil || location.horizontalAccuracy < 0) {
        // 無効なサンプル（CoreLocation の契約上 `horizontalAccuracy < 0` は未確定）
        // は push しない。
        return;
    }
    fandhe_rust_location_fix(location.coordinate.latitude, location.coordinate.longitude,
                              location.horizontalAccuracy);
}

- (void)locationManager:(CLLocationManager *)manager didFailWithError:(NSError *)error {
    if ([error.domain isEqualToString:kCLErrorDomain] && error.code == kCLErrorDenied) {
        fandhe_rust_location_failure(1);
        return;
    }
    if ([error.domain isEqualToString:kCLErrorDomain] && error.code == kCLErrorLocationUnknown) {
        // Apple の契約: 一時的な取得不能で、OS が更新を継続する。無視して待つ。
        return;
    }
    fandhe_rust_location_failure(3);
}

@end

static FandheLocationDelegate *g_location_delegate = nil;

static void FandheStartLocationOnMainThread(void) {
    dispatch_once(&g_location_manager_once, ^{
        g_location_manager = [[CLLocationManager alloc] init];
        g_location_delegate = [[FandheLocationDelegate alloc] init];
        g_location_manager.delegate = g_location_delegate;
    });
    FandheEvaluateAuthorization(g_location_manager);
}

void fandhe_apple_location_start(void) {
    if ([NSThread isMainThread]) {
        FandheStartLocationOnMainThread();
        return;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        FandheStartLocationOnMainThread();
    });
}
