// iOS 向け Web 認証セッション（Issue #532、親 #113）。
// ASWebAuthenticationSession でデモ認可ページ（127.0.0.1）を OS の認証 UI に出し、返ってきた
// コールバック URL を Rust へ渡す。Rust 側は src/device/ios_web_auth_session.rs。
// macOS 版（macos/fandhe_macos_web_auth_session.m）と同じ形で、違いは表示先（前面 scene の
// key window）だけ。
//
// C ABI の契約（Rust 側の SAFETY コメントの根拠）:
//   int32_t fandhe_ios_web_auth_start(uint64_t request_id, const char *url, const char *callback_scheme)
//     main thread から呼ぶ。戻り値: 0=開始 / 1=開始不可（表示先なし・start が NO）/ 2=入力不正・
//     main 以外・別のセッションが残っている。url / callback_scheme は呼び出しの間だけ有効（保持しない）。
//   void fandhe_ios_web_auth_cancel(uint64_t request_id)
//     任意スレッド（main 以外は main queue へ寄せる）。現在の request_id と一致するときだけ
//     セッションを閉じる（冪等）。
//   int32_t fandhe_ios_web_auth_random(uint8_t *buf, size_t len)   常に 0（arc4random_buf）
//   void fandhe_ios_web_auth_sha256(const uint8_t *data, size_t len, uint8_t out[32])
//
// 完了は main queue で 1 回だけ fandhe_rust_web_auth_session_complete(request_id, kind, as_code, url)
// を呼ぶ。kind 0=URL あり / 1=ASWebAuthenticationSessionErrorDomain のエラー（as_code にコード）/
// 2=その他。URL・エラー文はログに出さない。

#import <AuthenticationServices/AuthenticationServices.h>
#import <CommonCrypto/CommonDigest.h>
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#include <stdint.h>
#include <stdlib.h>

#import "fandhe_ios_common.h"

extern void fandhe_rust_web_auth_session_complete(uint64_t request_id, int32_t kind,
                                                  int64_t as_code, const char *url);

// 表示先は start 時に確定させ、nonnull の戻り値に nil を返さない。
@interface FandheWebAuthProvider : NSObject <ASWebAuthenticationPresentationContextProviding>
@property(nonatomic, strong) UIWindow *anchor;
@end

@implementation FandheWebAuthProvider
- (ASPresentationAnchor)presentationAnchorForWebAuthenticationSession:
    (ASWebAuthenticationSession *)session {
  return self.anchor;
}
@end

// main thread だけが触る。完了・取り消し・開始失敗のどれでも手放す。
static ASWebAuthenticationSession *gSession = nil;
static FandheWebAuthProvider *gProvider = nil;
static uint64_t gRequestId = 0;

int32_t fandhe_ios_web_auth_start(uint64_t request_id, const char *url,
                                  const char *callback_scheme) {
  if (![NSThread isMainThread] || url == NULL || callback_scheme == NULL || gSession != nil) {
    return 2;
  }
  @autoreleasepool {
    NSURL *nsurl = [NSURL URLWithString:[NSString stringWithUTF8String:url]];
    NSString *scheme = [NSString stringWithUTF8String:callback_scheme];
    if (nsurl == nil || scheme == nil) return 2;
    UIWindow *anchor = FandheRootViewController().view.window;
    if (anchor == nil) return 1;
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    ASWebAuthenticationSession *session = [[ASWebAuthenticationSession alloc]
              initWithURL:nsurl
        callbackURLScheme:scheme
        completionHandler:^(NSURL *callbackURL, NSError *error) {
          dispatch_async(dispatch_get_main_queue(), ^{
            @autoreleasepool {
              // 取り消し済み・別の要求に差し替わった後の完了は捨てる。
              if (gRequestId != request_id || gSession == nil) return;
              // 届ける前に状態を手放す。
              gSession = nil;
              gProvider = nil;
              gRequestId = 0;
              if (callbackURL != nil && error == nil) {
                fandhe_rust_web_auth_session_complete(request_id, 0, 0,
                                                      callbackURL.absoluteString.UTF8String);
              } else if (error != nil &&
                         [error.domain isEqualToString:ASWebAuthenticationSessionErrorDomain]) {
                fandhe_rust_web_auth_session_complete(request_id, 1, (int64_t)error.code, NULL);
              } else {
                fandhe_rust_web_auth_session_complete(request_id, 2, 0, NULL);
              }
            }
          });
        }];
#pragma clang diagnostic pop
    // Cookie を残さない。
    session.prefersEphemeralWebBrowserSession = YES;
    FandheWebAuthProvider *provider = [[FandheWebAuthProvider alloc] init];
    provider.anchor = anchor;
    session.presentationContextProvider = provider;
    gProvider = provider;
    gSession = session;
    gRequestId = request_id;
    if (![session start]) {
      gSession = nil;
      gProvider = nil;
      gRequestId = 0;
      return 1;
    }
  }
  return 0;
}

void fandhe_ios_web_auth_cancel(uint64_t request_id) {
  if (![NSThread isMainThread]) {
    dispatch_async(dispatch_get_main_queue(), ^{
      fandhe_ios_web_auth_cancel(request_id);
    });
    return;
  }
  if (gSession != nil && gRequestId == request_id) {
    // 完了ハンドラ（CanceledLogin）は request_id 不一致で捨てられるので、状態を先に手放して
    // 直後の再開始を妨げない。
    ASWebAuthenticationSession *session = gSession;
    gSession = nil;
    gProvider = nil;
    gRequestId = 0;
    [session cancel];
  }
}

int32_t fandhe_ios_web_auth_random(uint8_t *buf, size_t len) {
  arc4random_buf(buf, len);
  return 0;
}

void fandhe_ios_web_auth_sha256(const uint8_t *data, size_t len, uint8_t out[32]) {
  CC_SHA256(data, (CC_LONG)len, out);
}
