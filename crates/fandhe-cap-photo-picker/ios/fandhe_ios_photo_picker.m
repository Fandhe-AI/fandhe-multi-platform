// Phase 11（端末機能の拡張ループ、親 #1101）: iOS 側の実写真選択実装（Issue #1104）。
//
// `crates/fandhe-cap-photo-picker/src/ios.rs`（`present_ios_photo_picker`/
// `fandhe_rust_photo_picker_outcome`）から C ABI 経由で呼ばれる。`PHPickerViewController`
// は Rust バインディングを持たないため、この 1 ファイルの ObjC コードに実装を閉じ、
// `unsafe extern "C"` 宣言 1 点のみを Rust 側へ公開する（`fandhe_ios_file_picker.m` と同型。
// coding-rust.md の FFI 例外規定）。
//
// `build.rs` が iOS ターゲット（`CARGO_CFG_TARGET_OS == "ios"`）かつ feature `device`
// 有効時のみ `cc` でコンパイルし、PhotosUI・ImageIO をリンクする。
//
// 権限: 不要。`PHPickerViewController` はプロセス外のシステムピッカーで、選ばれた 1 枚だけが
// アプリへ渡る（`NSPhotoLibraryUsageDescription` は追加しない）。
//
// 契約（`ios.rs` のコメントと対）:
//   fandhe_ios_photo_picker_pick(requestId) -> 0 で「提示を依頼できた」。結果は非同期に
//   fandhe_rust_photo_picker_outcome(requestId, code, ...) 経由で 1 度だけ届く。
//   iOS は常に 0 を返し、提示失敗（top view controller 不在等）も code 2 で非同期に通知する。
//
//   fandhe_rust_photo_picker_outcome の code:
//     0 = 選択（path_ptr/path_len が一時ファイルの絶対パス、UTF-8）
//     1 = ユーザーがキャンセル（失敗ではない）
//     2 = 内部エラー（提示失敗・読み出し失敗・JPEG への変換失敗・パスの UTF-8 変換失敗等）
//     3 = busy（進行中のピッカーがある間の新しい要求。Rust 側は `Internal("photo_picker busy")` へ写す）
//   0 以外では path_ptr を参照しない（NULL を渡す）。
//
// 一時ファイルの寿命: Rust 側は呼び出しの中で読み・8 MiB 判定・復号確認まで同期に終える。
// `loadFileRepresentation` の URL は完了ハンドラの終了後に OS が消すため、ハンドラの中で
// 渡し切る（コピーを作らない）。JPEG / PNG 以外（HEIC・GIF・WebP 等）は ImageIO で JPEG へ
// 変換した自前の一時ファイルを渡し、成功・失敗のどちらでも呼び出しの後に削除する。
// 形式の最終判定は OS の名乗る型ではなく Rust 側 `validate_photo`（先頭バイト）が行う。
//
// スレッド: UIKit は main 必須のため提示は `dispatch_async(main)`。完了ハンドラは
// `NSItemProvider` のバックグラウンドキューで走り、Rust 側の復号もそこで行う（main を塞がない）。
// すべての Cocoa 呼び出しは `@autoreleasepool` の内側（Rust ワーカーではなく本ファイルの
// キュー上で動くが、ハンドラごとに明示する）。
//
// 進行中かどうかは OS の観測（提示中のピッカーが実際に提示されているか）で決める。推測タイマーのフラグは持たない:
// 画面 effect も公開入口（`begin_photo_pick`）も同じこの入口を通るため、Rust 側が slot を捨てた（`discard_photo_pick`）後でも、
// ピッカーが残っている間の新しい要求はここで busy（code 3）にする。

#import <ImageIO/ImageIO.h>
#import <PhotosUI/PhotosUI.h>
#import <UIKit/UIKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#import "fandhe_ios_common.h"

// 提示中の picker.delegate（weak）を生かしておくための強参照（ARC 管理）。main queue からのみ
// 読み書きする（`fandhe_ios_file_picker.m` の `g_retainedDelegate` と同じ理由）。
static id _Nullable g_retainedDelegate = nil;
// 提示中のピッカー（weak）。delegate が残っていてもピッカーが提示されていなければ、完了通知が来ないまま消えた（シーン破棄等）ので
// 進行中とみなさない（永久に busy にならないため）。main queue からのみ読み書きする。
static __weak PHPickerViewController *g_activePicker = nil;

// `ios.rs::fandhe_rust_photo_picker_outcome`（`fandhe-cap-photo-picker`）の宣言（本ファイルの唯一の呼び出し先）。
extern void fandhe_rust_photo_picker_outcome(uint64_t requestId, int32_t code,
                                              const uint8_t *_Nullable path_ptr, size_t path_len);

// url のパスを Rust へ渡す。UTF-8 化できなければ code 2。Rust 側が戻るまでに読み切る契約。
static void FandheDeliverPhotoPath(uint64_t requestId, NSURL *url) {
    NSData *pathData = [url.path dataUsingEncoding:NSUTF8StringEncoding];
    if (pathData == nil) {
        fandhe_rust_photo_picker_outcome(requestId, 2, NULL, 0);
        return;
    }
    fandhe_rust_photo_picker_outcome(requestId, 0, (const uint8_t *)pathData.bytes,
                                      pathData.length);
}

// 変換前の元データに許す上限（バイト）と画像寸法。Rust 側 `MAX_PHOTO_BYTES` /
// `MAX_PHOTO_SIDE` / `MAX_PHOTO_PIXELS` と揃える。変換後の長辺の目標は 4096 画素。
static const unsigned long long kFandhePhotoMaxBytes = 8ull * 1024 * 1024;
static const double kFandhePhotoMaxSide = 16384;
static const double kFandhePhotoMaxPixels = 64.0 * 1024 * 1024;
static const int kFandhePhotoConvertSide = 4096;

// 元データが上限を超えるか。ローカル一時ファイルなので実サイズを属性から読む（取得失敗は超過扱い）。
static BOOL FandhePhotoExceedsBytes(NSURL *src) {
    NSDictionary *attrs = [NSFileManager.defaultManager attributesOfItemAtPath:src.path error:nil];
    return attrs == nil || attrs.fileSize > kFandhePhotoMaxBytes;
}

// JPEG 以外の画像を JPEG の一時ファイルへ変換する。失敗は nil（呼び出し側が code 2 にする）。
// 呼び出し側が戻り値の URL を使い終えたら削除する。
static NSURL *_Nullable FandheConvertToJpeg(NSURL *src) {
    CGImageSourceRef source = CGImageSourceCreateWithURL((__bridge CFURLRef)src, NULL);
    if (source == NULL) {
        return nil;
    }
    // 復号前に寸法・画素数を検査する（ImageIO のヘッダ情報のみ。復号はしない）。
    NSDictionary *props = CFBridgingRelease(CGImageSourceCopyPropertiesAtIndex(source, 0, NULL));
    double w = [props[(__bridge NSString *)kCGImagePropertyPixelWidth] doubleValue];
    double h = [props[(__bridge NSString *)kCGImagePropertyPixelHeight] doubleValue];
    if (w <= 0 || h <= 0 || w > kFandhePhotoMaxSide || h > kFandhePhotoMaxSide ||
        w * h > kFandhePhotoMaxPixels) {
        CFRelease(source);
        return nil;
    }
    NSDictionary *thumbOpts = @{
        (__bridge NSString *)kCGImageSourceCreateThumbnailFromImageAlways : @YES,
        (__bridge NSString *)kCGImageSourceCreateThumbnailWithTransform : @YES,
        (__bridge NSString *)kCGImageSourceThumbnailMaxPixelSize : @(kFandhePhotoConvertSide),
    };
    // 長辺を 4096 以下へ縮めた画像だけを復号する（メモリ量を上限内に保つ）。
    CGImageRef image = CGImageSourceCreateThumbnailAtIndex(source, 0, (__bridge CFDictionaryRef)thumbOpts);
    CFRelease(source);
    if (image == NULL) {
        return nil;
    }
    NSString *name = [NSString stringWithFormat:@"fandhe-photo-%@.jpg", NSUUID.UUID.UUIDString];
    NSURL *dst = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:name]];
    CGImageDestinationRef dest = CGImageDestinationCreateWithURL(
        (__bridge CFURLRef)dst, (__bridge CFStringRef)UTTypeJPEG.identifier, 1, NULL);
    if (dest == NULL) {
        CGImageRelease(image);
        return nil;
    }
    NSDictionary *options = @{(__bridge NSString *)kCGImageDestinationLossyCompressionQuality : @0.9};
    CGImageDestinationAddImage(dest, image, (__bridge CFDictionaryRef)options);
    BOOL ok = CGImageDestinationFinalize(dest);
    CFRelease(dest);
    CGImageRelease(image);
    if (!ok) {
        [NSFileManager.defaultManager removeItemAtURL:dst error:nil];
        return nil;
    }
    return dst;
}

// 選ばれた 1 件を Rust へ渡す。JPEG / PNG はそのまま、それ以外の画像は JPEG へ変換して渡す。
static void FandheDeliverPickedProvider(uint64_t requestId, NSItemProvider *provider) {
    NSString *typeId = nil;
    BOOL needsConvert = NO;
    if ([provider hasItemConformingToTypeIdentifier:UTTypeJPEG.identifier]) {
        typeId = UTTypeJPEG.identifier;
    } else if ([provider hasItemConformingToTypeIdentifier:UTTypePNG.identifier]) {
        typeId = UTTypePNG.identifier;
    } else if ([provider hasItemConformingToTypeIdentifier:UTTypeImage.identifier]) {
        typeId = UTTypeImage.identifier;
        needsConvert = YES;
    } else {
        fandhe_rust_photo_picker_outcome(requestId, 2, NULL, 0);
        return;
    }

    [provider loadFileRepresentationForTypeIdentifier:typeId
                                    completionHandler:^(NSURL *_Nullable url,
                                                        NSError *_Nullable error) {
      @autoreleasepool {
          if (url == nil || error != nil) {
              fandhe_rust_photo_picker_outcome(requestId, 2, NULL, 0);
              return;
          }
          if (!needsConvert) {
              // この URL はハンドラの終了後に OS が消す。Rust がここで読み切る。
              FandheDeliverPhotoPath(requestId, url);
              return;
          }
          if (FandhePhotoExceedsBytes(url)) {
              // 元データが上限超: 変換せず原本を渡し、Rust 側の上限判定に InvalidInput で拒否させる。
              FandheDeliverPhotoPath(requestId, url);
              return;
          }
          NSURL *converted = FandheConvertToJpeg(url);
          if (converted == nil) {
              fandhe_rust_photo_picker_outcome(requestId, 2, NULL, 0);
              return;
          }
          FandheDeliverPhotoPath(requestId, converted);
          [NSFileManager.defaultManager removeItemAtURL:converted error:nil];
      }
    }];
}

// `PHPickerViewControllerDelegate` を実装し、結果を Rust へ橋渡しする。
// `UIAdaptivePresentationControllerDelegate` も併せて実装し、スワイプ dismiss もキャンセルとして
// 扱う。重複して届いた通知は Rust 側の受け取りの登録表（`pending.rs`。未登録・消費済みの id は無視）に
// 一任する（`fandhe_ios_file_picker.m` と同じ）。
@interface FandhePhotoPickerDelegate : NSObject <PHPickerViewControllerDelegate, UIAdaptivePresentationControllerDelegate>
@property(nonatomic, assign) uint64_t requestId;
@end

@implementation FandhePhotoPickerDelegate

- (void)picker:(PHPickerViewController *)picker didFinishPicking:(NSArray<PHPickerResult *> *)results {
    // programmatic dismiss が後から `presentationControllerDidDismiss:` を呼んで成功済みの結果を
    // キャンセルへ化けさせる競合を遮断するため、先に delegate を外す。
    picker.presentationController.delegate = nil;
    uint64_t requestId = self.requestId;
    // 以降 `self` に触れないこと（最後の強参照が落ちる）。重複して届いた旧 delegate の通知が、次の要求の delegate を
    // 外さないよう、自分が現在の delegate のときだけ外す。
    if (g_retainedDelegate == self) {
        g_retainedDelegate = nil;
    }
    // PHPicker は選択後に自動では閉じない。
    [picker dismissViewControllerAnimated:YES completion:nil];

    PHPickerResult *first = results.firstObject;
    if (first == nil) {
        fandhe_rust_photo_picker_outcome(requestId, 1, NULL, 0);
        return;
    }
    FandheDeliverPickedProvider(requestId, first.itemProvider);
}

- (void)presentationControllerDidDismiss:(UIPresentationController *)presentationController {
    uint64_t requestId = self.requestId;
    if (g_retainedDelegate == self) {
        g_retainedDelegate = nil;
    }
    fandhe_rust_photo_picker_outcome(requestId, 1, NULL, 0);
}

@end

int32_t fandhe_ios_photo_picker_pick(uint64_t requestId) {
    dispatch_async(dispatch_get_main_queue(), ^{
      @autoreleasepool {
        // 進行中のピッカー（提示中、提示・dismiss のアニメーション中）があれば、重ねて提示せず busy を返す。
        // 判定は delegate の有無ではなく OS の観測（提示状態）で行う: 完了通知で delegate を解放した直後も、
        // ピッカーは dismiss が終わるまで提示中のままで、その上へ present すると失敗してコールバックが来ない。
        PHPickerViewController *active = g_activePicker;
        if (active != nil && (active.presentingViewController != nil || active.isBeingPresented || active.isBeingDismissed)) {
            fandhe_rust_photo_picker_outcome(requestId, 3, NULL, 0);
            return;
        }
        // delegate だけ残っていれば、完了通知を失った残り（シーン破棄等）なので解放して続ける。
        // 解放する旧 delegate の要求には失敗（code 2）を 1 度だけ届ける（画面が `Picking` のまま、公開入口が未確定のまま止まらない）。
        // 旧 delegate は通知経路から外れているため、ここでの通知は重複しない。
        FandhePhotoPickerDelegate *stale = g_retainedDelegate;
        g_retainedDelegate = nil;
        g_activePicker = nil;
        if (stale != nil) {
            fandhe_rust_photo_picker_outcome(stale.requestId, 2, NULL, 0);
        }
        UIViewController *top = FandheTopmostViewController();
        // `top.view.window == nil` は `presentViewController` が delegate を一切呼ばずに失敗する
        // 既知の失敗系（`fandhe_ios_file_picker.m` と同じ）。提示成否はここで一度だけ決める。
        if (top == nil || top.view.window == nil) {
            fandhe_rust_photo_picker_outcome(requestId, 2, NULL, 0);
            return;
        }

        // `top`（最前面）が遷移中（提示の途中・dismiss の途中）なら、`presentViewController` は黙って捨てられ delegate が呼ばれない。
        // 遷移中だけ重ねて提示せず busy で返す（この要求にはまだ delegate を持たせていない）。安定して出ている別の画面
        // （別機能の system picker を含む。ファイル選択・色選択など）の上には重ねる: UIKit として正当で、そちらの進行中の判定は
        // 各機能が自分のピッカーについてだけ持つ（本機能は他機能のピッカーを観測しない）。
        // `FandheTopmostViewController` は一番奥の提示まで辿るため、`top.presentedViewController` は常に nil で判定に使えない。
        if (top.isBeingPresented || top.isBeingDismissed) {
            fandhe_rust_photo_picker_outcome(requestId, 3, NULL, 0);
            return;
        }

        PHPickerConfiguration *config = [[PHPickerConfiguration alloc] init];
        config.selectionLimit = 1;
        config.filter = [PHPickerFilter imagesFilter];
        PHPickerViewController *picker = [[PHPickerViewController alloc] initWithConfiguration:config];

        FandhePhotoPickerDelegate *delegate = [[FandhePhotoPickerDelegate alloc] init];
        delegate.requestId = requestId;
        // `picker.delegate` は弱参照のため明示的に保持する。
        picker.delegate = delegate;
        picker.presentationController.delegate = delegate;
        g_retainedDelegate = delegate;
        g_activePicker = picker;

        // 提示の成否は completion（提示の完了後、presented 側の `viewDidAppear:` の後に呼ばれる。公式文書）で確かめる。
        // 直後には確かめない: `presentingViewController` / `isBeingPresented` が提示の直後に立つ保証が文書に無く、立つ前に確かめると
        // 正しい提示を失敗と誤判定して delegate を外し、選択を捨ててしまう。ここは「completion が来たのに提示が成立していない」
        // 場合の安全網で、通常は何もしない。delegate の完了経路が先に走っていれば（`g_retainedDelegate != delegate`）何もしない。
        // 既知の天井: UIKit が提示を捨てたとき completion が呼ばれるかは文書に無く、呼ばれなければこの要求は確定しない。
        // completion ブロックが picker を強参照するため、weak の `g_activePicker` は nil にならない。残った delegate は、次の要求が
        // 「提示されていない（`presentingViewController == nil && !isBeingPresented && !isBeingDismissed`）」と判定してここで
        // 旧 delegate を失敗（code 2）で確定して回収する（推測のタイマーは持たない）。ただし Rust 側の slot は要求中のまま残るため、
        // 次の要求がここへ届くのは、公開入口で discard したあと、または slot が reset されたあとだけ（画面 effect は busy で断られ、
        // iOS に画面の `Picking` を抜ける経路は無い）。遷移中は上で busy にして提示自体を避けるので、捨てられること自体はほぼ起きない前提の天井。
        [top presentViewController:picker
                          animated:YES
                        completion:^{
                          if (g_retainedDelegate == delegate && picker.presentingViewController == nil) {
                              g_retainedDelegate = nil;
                              g_activePicker = nil;
                              picker.presentationController.delegate = nil;
                              fandhe_rust_photo_picker_outcome(requestId, 2, NULL, 0);
                          }
                        }];
      }
    });

    return 0;
}
