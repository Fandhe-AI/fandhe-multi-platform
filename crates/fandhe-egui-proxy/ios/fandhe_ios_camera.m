// Phase 8（端末機能の型検証、Issue #751）: iOS 側の実カメラ実装。
//
// `crates/fandhe-egui-proxy/src/device/ios.rs`（`IosCamera`）から C ABI 経由で
// 呼ばれる。UIKit（`UIImagePickerController`）・AVFoundation（権限確認）は Rust
// バインディングを持たないため、この 1 ファイルの ObjC コードに実装を閉じ、
// `unsafe extern "C"` 宣言 1 点のみを Rust 側へ公開する（`lib.rs` の
// iOS `deny(unsafe_code)` 緩和と対になる、coding-rust.md の FFI 例外規定）。
//
// `build.rs` が iOS ターゲット（`CARGO_CFG_TARGET_OS == "ios"`）かつ feature
// `device` 有効時のみ `cc` でコンパイルし、`fandhe_ios_camera` という静的ライブラリへ
// リンクする（他ターゲットのビルドには一切関与しない）。
//
// 契約（`ios.rs` のコメントと対）:
//   fandhe_ios_camera_capture(max_edge, &out_rgba, &out_w, &out_h) -> 0 で成功。
//   成功時のみ *out_rgba は width*height*4 バイトの RGBA8（行優先・パディングなし）を
//   指す malloc 済み領域で、呼び出し側が fandhe_ios_camera_free で解放する契約。
//   1 = カメラ非搭載（シミュレータ等）、2 = 権限拒否、3 = ユーザーキャンセル、
//   4 = 内部エラー（main スレッドから呼ばれた・二重呼び出し・画像変換失敗・
//   rootViewController 不在・提示失敗またはタイムアウト）。
//
// UIKit は main スレッド必須のため、呼び出し元（`ios.rs`、`fandhe-device` の
// `Camera::capture` 契約上ワーカースレッドから同期呼び出しされる）をこの関数内で
// ブロックし、`dispatch_async(main)` で UI 操作を実行してセマフォで完了を待つ。

#import <UIKit/UIKit.h>
#import <AVFoundation/AVFoundation.h>
#import <stdatomic.h>
#import "fandhe_ios_common.h"

// 二重呼び出し防止（このプロセスで同時に 1 回しか撮影 UI を出さない、ponytail:
// グローバルフラグでの単純化。複数カメラ画面の同時起動が要件化したら
// インスタンス単位のロックへ拡張する）。
static atomic_flag g_capture_in_flight = ATOMIC_FLAG_INIT;

// delegate/completion が一切呼ばれないまま停止する経路（Bugbot 指摘: 提示失敗時に
// dispatch_semaphore_wait が無期限ブロックし続ける）への最終防衛線。Android 側
// `CaptureMailbox` の待ち上限（`device/android.rs` の `CAPTURE_TIMEOUT`）と同値に揃える。
static const int64_t kFandheCameraCaptureTimeoutSeconds = 300;

// `UIImagePickerControllerDelegate` を実装し、結果を `__block` 変数へ橋渡しする。
@interface FandheCameraDelegate : NSObject <UIImagePickerControllerDelegate, UINavigationControllerDelegate>
@property(nonatomic, copy) void (^onFinish)(UIImage *_Nullable image, BOOL cancelled);
@end

@implementation FandheCameraDelegate
- (void)imagePickerController:(UIImagePickerController *)picker
    didFinishPickingMediaWithInfo:(NSDictionary<UIImagePickerControllerInfoKey, id> *)info {
    UIImage *image = info[UIImagePickerControllerOriginalImage];
    [picker dismissViewControllerAnimated:YES
                                completion:^{
                                  if (self.onFinish) {
                                    self.onFinish(image, NO);
                                  }
                                }];
}

- (void)imagePickerControllerDidCancel:(UIImagePickerController *)picker {
    [picker dismissViewControllerAnimated:YES
                                completion:^{
                                  if (self.onFinish) {
                                    self.onFinish(nil, YES);
                                  }
                                }];
}
@end

// `FandheTopmostViewController` は `fandhe_ios_common.h` へ移した（Issue #899 で
// 3 箇所目の利用（共有シート）が出たため、`fandhe_ios_file_picker.m` の申し送り通り
// 抽出。撮影 UI をそこから present するために使う（rootViewController 直下に別
// モーダルが出ている状態でも壊れないようにするため。scene 非対応の winit 製アプリ
// 向けフォールバックの詳細は `fandhe_ios_common.h` 側のコメント参照）。

// `UIImage` を長辺 `maxEdge` 以下へ縮小し、向きを正規化した RGBA8 バイト列を返す。
// `UIGraphicsImageRenderer` は描画時に `imageOrientation` を適用するため、
// 撮影直後の向き情報（EXIF 相当）をこの 1 回の描画で normalize できる。
static BOOL FandheEncodeImageRGBA(UIImage *image, uint32_t maxEdge, uint8_t *_Nullable *_Nonnull outRgba,
                                   uint32_t *outW, uint32_t *outH) {
    CGSize size = image.size;
    if (size.width <= 0 || size.height <= 0) {
        return NO;
    }
    CGFloat scale = MIN(1.0, (CGFloat)maxEdge / MAX(size.width, size.height));
    CGSize targetSize = CGSizeMake(MAX(1, round(size.width * scale)), MAX(1, round(size.height * scale)));

    UIGraphicsImageRendererFormat *format = [UIGraphicsImageRendererFormat preferredFormat];
    format.opaque = YES;
    format.scale = 1.0;
    UIGraphicsImageRenderer *renderer = [[UIGraphicsImageRenderer alloc] initWithSize:targetSize format:format];
    UIImage *resized = [renderer imageWithActions:^(UIGraphicsImageRendererContext *_Nonnull __unused ctx) {
      [image drawInRect:CGRectMake(0, 0, targetSize.width, targetSize.height)];
    }];

    CGImageRef cgImage = resized.CGImage;
    if (cgImage == NULL) {
        return NO;
    }
    uint32_t w = (uint32_t)CGImageGetWidth(cgImage);
    uint32_t h = (uint32_t)CGImageGetHeight(cgImage);
    if (w == 0 || h == 0) {
        return NO;
    }

    size_t byteLen = (size_t)w * (size_t)h * 4;
    uint8_t *buffer = (uint8_t *)malloc(byteLen);
    if (buffer == NULL) {
        return NO;
    }

    CGColorSpaceRef colorSpace = CGColorSpaceCreateDeviceRGB();
    // RGBA8・行優先・パディングなし（ios.rs 側の契約）に一致させるため、
    // bytesPerRow を width*4 に固定し、premultipliedLast + 32Big（既定バイト順）を使う。
    CGContextRef context = CGBitmapContextCreate(buffer, w, h, 8, (size_t)w * 4, colorSpace,
                                                  kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
    CGColorSpaceRelease(colorSpace);
    if (context == NULL) {
        free(buffer);
        return NO;
    }
    // CGBitmapContextCreate が確保するバッファは、UIKit の自動フリップ対象では
    // ないため、UIImage.CGImage を rect (0,0,w,h) へそのまま描画するだけで
    // バッファの row 0 が画像の視覚的な上端と一致する（ios.rs 側の契約）。
    // CTM を反転させると出力画素が上下逆になるため、追加のフリップは行わない
    // （PR #791 codex-review 指摘: 以前のフリップ追加は誤りで、上下反転を
    // 引き起こしていた）。
    CGContextDrawImage(context, CGRectMake(0, 0, w, h), cgImage);
    CGContextRelease(context);

    *outRgba = buffer;
    *outW = w;
    *outH = h;
    return YES;
}

int32_t fandhe_ios_camera_capture(uint32_t max_edge, uint8_t *_Nullable *_Nonnull out_rgba, uint32_t *out_w,
                                   uint32_t *out_h) {
    // main スレッドから呼ばれると、後段の dispatch_sync 待ちと自己デッドロックする
    // （呼び出し契約: ワーカースレッドから同期呼び出しされる、ios.rs 参照）。
    if ([NSThread isMainThread]) {
        return 4;
    }
    if (atomic_flag_test_and_set(&g_capture_in_flight)) {
        return 4;
    }

    __block int32_t resultCode = 4;
    __block UIImage *capturedImage = nil;
    // タイムアウト時に dismiss するため、提示中の picker を main キュー外から
    // 参照できるように保持しておく（下記 dispatch_semaphore_wait 参照）。
    __block UIImagePickerController *presentedPicker = nil;
    // delegate を保持する変数を関数スコープへ引き上げ、タイムアウト経路からも
    // nil 化できるようにする（PR #791 コードレビュー指摘 2: 従来は
    // presentPicker 内の __block 変数に閉じており、タイムアウト時に
    // delegate ⇄ onFinish ブロックの循環参照が解放されないままだった）。
    __block FandheCameraDelegate *retainedDelegate = nil;
    // この撮影試行が既に終了したかどうか。権限コールバック・delegate・
    // タイムアウトの 3 経路が同時に走り得るため、最初に確定した経路以外を
    // 弾く（PR #791 コードレビュー指摘 1）。すべて main キュー上でのみ
    // 読み書きするため（タイムアウト側も dispatch_sync(main) 経由）、
    // main キューの直列実行だけで排他が成立し追加の同期は不要。
    __block BOOL finished = NO;
    dispatch_semaphore_t doneSem = dispatch_semaphore_create(0);

    // 終了処理を一箇所に集約するクロージャ。二重終了防止（finished フラグ）と
    // delegate 循環参照の解放（retainedDelegate = nil）を全終了経路で
    // 一律に行う。呼び出しは必ず main キュー上から行うこと。
    void (^finishWith)(int32_t) = ^(int32_t code) {
      if (finished) {
          return;
      }
      finished = YES;
      retainedDelegate = nil;
      resultCode = code;
      dispatch_semaphore_signal(doneSem);
    };

    dispatch_async(dispatch_get_main_queue(), ^{
      if (![UIImagePickerController isSourceTypeAvailable:UIImagePickerControllerSourceTypeCamera]) {
          finishWith(1);
          return;
      }

      AVAuthorizationStatus authStatus = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo];
      if (authStatus == AVAuthorizationStatusDenied || authStatus == AVAuthorizationStatusRestricted) {
          finishWith(2);
          return;
      }

      void (^presentPicker)(void) = ^{
        // タイムアウトが既に確定していれば、権限コールバック経由で遅延到達
        // した場合でも UI を提示しない（PR #791 コードレビュー指摘 1）。
        if (finished) {
            return;
        }
        UIViewController *top = FandheTopmostViewController();
        // `top.view.window == nil`（view がウィンドウ階層に無い）場合は
        // `presentViewController` がデリゲートを一切呼ばずに失敗する経路
        // （PR #791 レビュー指摘: この場合デリゲート待ちのセマフォが永遠に
        // 解除されず capturing 状態のまま停止する）。呼ぶ前に判定して
        // 即座に内部エラーとして返す。`top.presentedViewController != nil`
        // は `FandheTopmostViewController` が既に最深部まで辿るため常に
        // 偽になり判定として無意味なため置かない（Bugbot 指摘）。
        if (top == nil || top.view.window == nil) {
            finishWith(4);
            return;
        }

        UIImagePickerController *picker = [[UIImagePickerController alloc] init];
        picker.sourceType = UIImagePickerControllerSourceTypeCamera;
        // カメラソースは iPad 上でフルスクリーン表示が必須（Automatic/page-sheet
        // のままだと iPad で壊れた UI・クラッシュの恐れがある、PR #791 レビュー
        // 指摘）。iPhone では既定でもフルスクリーンだが、明示して両機種で
        // 同一の見た目にする。
        picker.modalPresentationStyle = UIModalPresentationFullScreen;
        picker.allowsEditing = NO;
        FandheCameraDelegate *delegate = [[FandheCameraDelegate alloc] init];
        // ARC 管理下のオブジェクトを completion までブロック側で生かしておく
        // （delegate は picker.delegate から弱参照されないため、明示的に保持する）。
        picker.delegate = delegate;
        retainedDelegate = delegate;
        delegate.onFinish = ^(UIImage *_Nullable image, BOOL cancelled) {
          if (cancelled) {
              finishWith(3);
          } else if (image == nil) {
              finishWith(4);
          } else {
              capturedImage = image;
              finishWith(0);
          }
        };
        presentedPicker = picker;
        [top presentViewController:picker
                           animated:YES
                         completion:^{
                           // `top` が遷移中等で実際には提示できていない場合、
                           // `presentedViewController` は `picker` に一致しない
                           // （UIKit はこの completion をデリゲートより先に、
                           // 失敗時でも呼び得る）。この経路を検知しないと
                           // delegate が呼ばれずセマフォが解除されない
                           // （Bugbot 指摘）ため、ここで失敗を確定させる。
                           if (top.presentedViewController != picker) {
                               finishWith(4);
                           }
                         }];
      };

      if (authStatus == AVAuthorizationStatusNotDetermined) {
          [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo
                                    completionHandler:^(BOOL granted) {
                                      dispatch_async(dispatch_get_main_queue(), ^{
                                        // タイムアウト後に届いた遅延コールバックは
                                        // presentPicker() を実行させない（PR #791
                                        // コードレビュー指摘 1）。
                                        if (finished) {
                                            return;
                                        }
                                        if (!granted) {
                                            finishWith(2);
                                            return;
                                        }
                                        presentPicker();
                                      });
                                    }];
      } else {
          presentPicker();
      }
    });

    // 提示失敗の completion 検知（上記）で塞げない未知の経路（delegate も
    // completion も呼ばれないまま停止する場合）への最終防衛線として、
    // 無期限待ちではなく有限タイムアウトを設ける（Bugbot 指摘）。
    dispatch_time_t deadline =
        dispatch_time(DISPATCH_TIME_NOW, kFandheCameraCaptureTimeoutSeconds * NSEC_PER_SEC);
    if (dispatch_semaphore_wait(doneSem, deadline) != 0) {
        dispatch_sync(dispatch_get_main_queue(), ^{
          // finished を確定させ、後から届く権限コールバック・delegate 呼び出しを
          // 弾く（指摘 1）と同時に retainedDelegate を解放し、delegate ⇄
          // onFinish ブロックの循環参照を断つ（指摘 2）。
          finishWith(4);
          if (presentedPicker.presentingViewController != nil) {
              [presentedPicker dismissViewControllerAnimated:NO completion:nil];
          }
        });
        atomic_flag_clear(&g_capture_in_flight);
        return 4;
    }
    atomic_flag_clear(&g_capture_in_flight);

    if (resultCode != 0) {
        return resultCode;
    }
    if (capturedImage == nil) {
        return 4;
    }
    // `FandheEncodeImageRGBA` は `UIGraphicsImageRendererFormat.preferredFormat`
    // 等 UIKit のレンダラー API を呼ぶため main スレッド必須（`UIScreen` の
    // trait 読み取りが内部にある、PR #791 レビュー指摘）。この関数を呼ぶ
    // ワーカースレッド上で直接実行すると未定義動作の恐れがあるため、
    // dispatch_sync で main へ委譲し、@autoreleasepool で一時オブジェクトの
    // 解放を保証する。
    __block BOOL encodeOk = NO;
    dispatch_sync(dispatch_get_main_queue(), ^{
      @autoreleasepool {
        encodeOk = FandheEncodeImageRGBA(capturedImage, max_edge, out_rgba, out_w, out_h);
      }
    });
    if (!encodeOk) {
        return 4;
    }
    return 0;
}

void fandhe_ios_camera_free(uint8_t *_Nullable ptr) {
    free(ptr);
}
