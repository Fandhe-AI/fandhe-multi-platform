// Phase 11 の拡充ループ（親 #89）型サンプル「機械学習モデル推論」: macOS 実装（Issue #289）。
// iOS 実装（b-ios）と共用できるよう `TARGET_OS` では分けない（CoreML と Foundation だけを使い、
// UIKit / AppKit に触れない）。
//
// `crates/fandhe-egui-proxy/src/device/apple_ml_inference.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS・iOS ターゲットの feature `device` 有効時に `cc` コンパイルする
// （Rust 側 cfg と同じ OS 集合に保つ）。
//
// 契約（一発呼び出し型。結果を待つ要求は持たない）:
//   fandhe_apple_ml_inference_run(const uint8_t *spec, size_t len, const double *input, size_t n,
//                                 uint64_t deadline_ms, double *out_y) -> int32_t
//     spec/len   : Core ML の仕様（protobuf）のバイト列。呼び出し内でコピーし、戻った後は参照しない
//     input/n    : 入力 `x`（double の MultiArray、shape [n]）。呼び出しの間だけ参照する
//     deadline_ms: モデル読み込みの完了を待つ上限（Rust 側が残り時間から渡す）
//     out_y      : 成功時に出力 `y` を書く
//     0 = 成功
//     1 = モデルを拒否された（仕様を読めない・読み込みに失敗）。呼び出し元が INVALID_MODEL へ写す
//     2 = 推論の失敗（`predictionFromFeatures:` の失敗・`y` が無い・数値でない）
//     3 = 読み込みが deadline_ms 内に終わらなかった
//     4 = OS が古い（macOS 13 / iOS 16 未満。`MLModelAsset` が無い）。呼び出し元が Unsupported へ写す
//
// 計算ユニットは CPU に固定する（GPU・NPU の選択は扱わない）。
//
// スレッド: 呼ばれたスレッド（Rust のワーカー）で同期に実行する（`dispatch_sync` は使わない。
// main 側が呼び出し元を待つとデッドロックするため、`device-bridge-contracts.md`「11」）。
// Cocoa 呼び出しは `@autoreleasepool` の内側で行う。読み込みの完了ブロックは `__block` 変数だけを
// 触り、Rust のポインタを持ち込まない。期限切れで先に戻っても、遅れて走る完了ブロックは
// 自分のセマフォと結果だけを触って終わる。
//
// モデル・入力・出力・OS のエラー文はログへ出さない（security.md A09）。権限・entitlement は不要。

#import <CoreML/CoreML.h>
#import <Foundation/Foundation.h>

int32_t fandhe_apple_ml_inference_run(const uint8_t *spec, size_t len, const double *input,
                                      size_t n, uint64_t deadline_ms, double *out_y) {
    @autoreleasepool {
        if (@available(macOS 13.0, iOS 16.0, *)) {
            // コピーして Rust 側のバッファ寿命から切り離す: 期限切れで先に戻っても、遅れて走る
            // Core ML の読み込みが解放済みメモリを読まない（NoCopy は use-after-free になる）。
            NSData *data = [NSData dataWithBytes:spec length:len];
            NSError *assetError = nil;
            // クラス参照を実行時に引く: 配備下限（macOS 11）の端末で `MLModelAsset` のシンボル解決に
            // 失敗して起動時に落ちないようにする（`@available` と併せて OS が古ければ 4 を返す）。
            Class assetClass = NSClassFromString(@"MLModelAsset");
            if (assetClass == Nil) {
                return 4;
            }
            MLModelAsset *asset = [assetClass modelAssetWithSpecificationData:data
                                                                        error:&assetError];
            if (asset == nil) {
                return 1;
            }
            MLModelConfiguration *config = [[MLModelConfiguration alloc] init];
            config.computeUnits = MLComputeUnitsCPUOnly;

            dispatch_semaphore_t done = dispatch_semaphore_create(0);
            __block MLModel *loaded = nil;
            [MLModel loadModelAsset:asset
                      configuration:config
                  completionHandler:^(MLModel *model, NSError *__unused error) {
                      loaded = model;
                      dispatch_semaphore_signal(done);
                  }];
            dispatch_time_t limit =
                dispatch_time(DISPATCH_TIME_NOW, (int64_t)(deadline_ms * NSEC_PER_MSEC));
            if (dispatch_semaphore_wait(done, limit) != 0) {
                return 3;
            }
            if (loaded == nil) {
                return 1;
            }

            NSError *arrayError = nil;
            MLMultiArray *x = [[MLMultiArray alloc] initWithShape:@[ @(n) ]
                                                         dataType:MLMultiArrayDataTypeDouble
                                                            error:&arrayError];
            if (x == nil) {
                return 2;
            }
            for (size_t i = 0; i < n; i++) {
                x[i] = @(input[i]);
            }
            NSError *featError = nil;
            MLDictionaryFeatureProvider *features =
                [[MLDictionaryFeatureProvider alloc] initWithDictionary:@{@"x" : x}
                                                                  error:&featError];
            if (features == nil) {
                return 2;
            }
            NSError *predError = nil;
            id<MLFeatureProvider> out = [loaded predictionFromFeatures:features error:&predError];
            MLFeatureValue *y = [out featureValueForName:@"y"];
            if (out == nil || y == nil || y.type != MLFeatureTypeDouble) {
                return 2;
            }
            *out_y = y.doubleValue;
            return 0;
        }
        return 4;
    }
}
