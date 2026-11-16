// Phase 11 の拡充ループ（親 #1121）型サンプル「写真の保存」: macOS 実装（Issue #1254）と
// iOS 実装（Issue #1255）。Photos の API が iOS / macOS で同じで UIKit も要らないため、
// `#if TARGET_OS_IPHONE` 分岐なしで共用する。
//
// `crates/fandhe-egui-proxy/src/device/apple_photo_library.rs` から C ABI 経由で呼ばれる。
// `build.rs` の macOS / iOS 両分岐が feature `device` 有効時に `cc` でコンパイルし、Photos を
// リンクする。
//
// 契約（pull 型。`fandhe_apple_calendar.m` と同型、coding-rust.md の FFI 例外規定）:
//   fandhe_apple_photo_library_save(uint64_t request_id, const uint8_t *bytes, size_t len)
//       -> int32_t
//     戻り値: 0 = 依頼を受理（結果は非同期に fandhe_rust_photo_library_outcome で届く）、
//     1 = 受理できない（NULL / 長さ 0 / NSData を作れない）。bytes は PNG のバイト列で、
//     return する前に NSData へ値コピーするため、呼び出し元は return 後に解放してよい
//     （所有権は移らない）。
//   fandhe_rust_photo_library_outcome(uint64_t request_id, int32_t code)
//     Rust 側が公開する関数。code は device/photo_save.rs::outcome_from_apple_code の表が SSOT
//     （0 保存確定 / 1 拒否・制限・未決定のまま / 2 macOS 11 未満 / 3 performChanges の失敗 /
//      4 認可要求の異常）。各依頼につきちょうど 1 回だけ呼ぶ。
//
// 権限は add-only（PHAccessLevelAddOnly）だけを求める。フルアクセスは求めない
// （Info.plist は iOS・macOS とも NSPhotoLibraryAddUsageDescription のみ）。Authorized / Limited なら保存へ進み、
// Denied / Restricted / NotDetermined は code 1。保存の成否は performChanges の
// success かつ error == nil のときだけ code 0（成功を作らない）。
//
// スレッド: Photos への操作は専用 serial queue に閉じる（契約表 11）。許可ダイアログは OS が
// 出す（こちらから UI を出さない）ため main queue は不要。各ブロックは @autoreleasepool で
// 包む（契約表 5）。
//
// 画像のバイト列・NSError の内容はログへ出さない（NSLog 不使用、security.md A09）。
// 実機での動作確認（TCC ダイアログ・写真 App に入ること・拒否時に成功が出ないこと）は #1246（人手）。

#import <Foundation/Foundation.h>
#import <Photos/Photos.h>

extern void fandhe_rust_photo_library_outcome(uint64_t request_id, int32_t code);

static dispatch_queue_t FandhePhotoLibraryQueue(void) {
    static dispatch_queue_t q;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      q = dispatch_queue_create("com.fandhe.egui-proxy.photo-library", DISPATCH_QUEUE_SERIAL);
    });
    return q;
}

API_AVAILABLE(macos(11.0), ios(14.0))
static void FandhePerformSave(uint64_t request_id, NSData *data) {
    [[PHPhotoLibrary sharedPhotoLibrary]
        performChanges:^{
          PHAssetCreationRequest *req = [PHAssetCreationRequest creationRequestForAsset];
          [req addResourceWithType:PHAssetResourceTypePhoto data:data options:nil];
        }
        completionHandler:^(BOOL success, NSError *_Nullable error) {
          // completion は任意キューから来る。serial queue へ戻して一度だけ通知する。
          dispatch_async(FandhePhotoLibraryQueue(), ^{
            @autoreleasepool {
              int32_t code;
              if (success && error == nil) {
                  code = 0;
              } else if (error != nil && [error.domain isEqualToString:PHPhotosErrorDomain] &&
                         (error.code == PHPhotosErrorAccessUserDenied ||
                          error.code == PHPhotosErrorAccessRestricted)) {
                  code = 1;
              } else {
                  code = 3;
              }
              fandhe_rust_photo_library_outcome(request_id, code);
            }
          });
        }];
}

int32_t fandhe_apple_photo_library_save(uint64_t request_id, const uint8_t *bytes, size_t len) {
    if (bytes == NULL || len == 0) {
        return 1;
    }
    // 返る前に値コピーする（呼び出し元は return 後に bytes を解放する）。
    NSData *data = [NSData dataWithBytes:bytes length:len];
    if (data == nil) {
        return 1;
    }

    dispatch_async(FandhePhotoLibraryQueue(), ^{
      @autoreleasepool {
        if (@available(macOS 11.0, iOS 14.0, *)) {
            [PHPhotoLibrary
                requestAuthorizationForAccessLevel:PHAccessLevelAddOnly
                                           handler:^(PHAuthorizationStatus status) {
                                             dispatch_async(FandhePhotoLibraryQueue(), ^{
                                               @autoreleasepool {
                                                 if (status == PHAuthorizationStatusAuthorized ||
                                                     status == PHAuthorizationStatusLimited) {
                                                     FandhePerformSave(request_id, data);
                                                 } else {
                                                     fandhe_rust_photo_library_outcome(request_id, 1);
                                                 }
                                               }
                                             });
                                           }];
        } else {
            fandhe_rust_photo_library_outcome(request_id, 2);
        }
      }
    });
    return 0;
}
