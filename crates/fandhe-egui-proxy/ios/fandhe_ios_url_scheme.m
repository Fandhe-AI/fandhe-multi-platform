// Phase 11 の拡充ループ（親 #1116）型サンプル「カスタム URL scheme」: iOS 実装
// （Issue #1217）。`fandhe://` で起動したとき（コールドスタート）と、起動済みのアプリが
// URL を受けたとき（ウォーム）の両方を、共通コアの検査（`UrlInbox::receive_bytes`）へ流す。
//
// `crates/fandhe-egui-proxy/src/device/apple_url_scheme.rs`（macOS と共用）から C ABI 経由で
// 呼ばれる。`build.rs` の `build_ios()` が feature `device` 有効時にのみコンパイルする。
// Info.plist 側の宣言は `platforms/apple/FandheIosApp/Resources/Info.plist` の
// `CFBundleURLTypes`。
//
// ファイル URL（`file://`）は `apple/fandhe_apple_file_association.m` が受けるため、ここでは無視する。
//
// 経路は 2 つ。(1) Info.plist の `UIApplicationSceneManifest`（Issue #27）で scene 形態になった今は、
// `fandhe-platform-apple` の scene delegate が受けた URL（`connectionOptions.URLContexts` と
// `scene:openURLContexts:`）を通知 `FandheSceneOpenURLNotification` で投げ、ここで受ける。
// (2) 旧来経路: scene 形態でない場合の `application:openURL:options:` は、共通層の application delegate
// （`fandhe-platform-apple` の `ios/fandhe_ios_app_delegate.m`、Issue #1325）が同じ通知へ合流させる。
// 本ファイルは `fandhe_ios_app_delegate_install()` で delegate の差し込みを依頼するだけで、delegate は持たない。
// 背景ダウンロード・背景送信の `handleEventsForBackgroundURLSession` は、各機能が
// `fandhe_ios_register_background_session` で登録して直接受ける（本ファイルは関与しない）。
//
// 契約:
//   fandhe_apple_url_scheme_register(void) -> void
//     main thread から `UIApplicationMain` より前に呼ぶ。冪等。起動通知と scene 通知の observer は
//     static の強参照で持ち続ける。
//   fandhe_rust_url_scheme_received(const uint8_t *ptr, size_t len) -> Rust 側
//     （`apple_url_scheme.rs`）が公開する `extern "C"` 関数。URL を UTF-8 で渡す。
//     文字列が取れなかったときは (NULL, 0)。ポインタは呼び出し中だけ有効。
//
// スレッド契約: 起動通知は queue:nil で投稿スレッド（main）の同期実行、他は mainQueue で受け、UIKit も delegate を main thread から呼ぶ。
// 中身は `@autoreleasepool` で囲み、`g_launch_url` は main thread だけが触る
// （`device-bridge-contracts.md`「5. FFI 境界」）。
//
// 二重転送の防止: 起動時の URL を launchOptions から転送した後、UIKit が同じ URL で
// `application:openURL:options:` も呼ぶかは OS 次第。`g_launch_url` と一致する最初の 1 回だけ
// 捨てて、同じ URL を 2 回数えない。DidBecomeActive で記憶を捨てる。
//
// URL は秘密値を含みうるためログへ出さない（NSLog を使わない）。
// アプリ終了時: プロセスが終われば登録も消えるため、明示的な解放はしない。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

#import "fandhe_ios_app_events.h"

extern void fandhe_rust_url_scheme_received(const uint8_t *ptr, size_t len);

static void fandhe_forward_url(NSURL *url) {
  // `fandhe://web-auth/*` は Web 認証セッション（`fandhe_ios_web_auth_session.m`）の専用経路。
  // セッション内のコールバックは ASWebAuthenticationSession が横取りするため openURL には来ず、
  // ここへ来るのはセッションの外（Safari 等）から開かれた場合だけで信頼しない。受信箱へ入れると
  // code・state が url-scheme 画面に出るため、拒否件数にも数えず捨てる（Issue #532）。
  // nil への caseInsensitiveCompare は 0（NSOrderedSame）を返すため、scheme・host の nil を先に除外する
  // （`fandhe:` のようなホスト無し URL は通常どおり受信箱へ転送する）。
  if (url.scheme != nil && url.host != nil &&
      [url.scheme caseInsensitiveCompare:@"fandhe"] == NSOrderedSame &&
      [url.host caseInsensitiveCompare:@"web-auth"] == NSOrderedSame) {
    return;
  }
  NSData *data = [url.absoluteString dataUsingEncoding:NSUTF8StringEncoding];
  if (data == nil || data.length == 0) {
    fandhe_rust_url_scheme_received(NULL, 0);
  } else {
    fandhe_rust_url_scheme_received((const uint8_t *)data.bytes, (size_t)data.length);
  }
}

static NSURL *g_launch_url = nil;

// 受けた URL を転送する。起動時に転送済みの同じ URL は 1 回だけ捨てる（二重転送の防止）。
static void fandhe_handle_open_url(NSURL *url) {
  // ファイル URL は `apple/fandhe_apple_file_association.m` が受ける（scheme 不一致の拒否に数えない）。
  if (url.isFileURL) {
    return;
  }
  NSURL *launched = g_launch_url;
  g_launch_url = nil;
  if (launched == nil || ![url isEqual:launched]) {
    fandhe_forward_url(url);
  }
}

static id g_launch_observer = nil;
static id g_active_observer = nil;
static id g_scene_observer = nil;

void fandhe_apple_url_scheme_register(void) {
  if (g_scene_observer != nil) {
    return;
  }
  fandhe_ios_app_delegate_install();
  g_scene_observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:FandheSceneOpenURLNotification
                  object:nil
                   queue:[NSOperationQueue mainQueue]
              usingBlock:^(NSNotification *note) {
                @autoreleasepool {
                  if ([note.object isKindOfClass:[NSURL class]]) {
                    fandhe_handle_open_url(note.object);
                  }
                }
              }];
  g_launch_observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:UIApplicationDidFinishLaunchingNotification
                  object:nil
                   // queue:nil は投稿スレッド（main）で同期実行される。起動時の URL を次の run loop まで待たせない。
                   queue:nil
              usingBlock:^(NSNotification *note) {
                @autoreleasepool {
                  // シーン無しのライフサイクルでは launchOptions が唯一の経路（scene 形態では nil）（iOS 26 で deprecated）。
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
                  id launched = note.userInfo[UIApplicationLaunchOptionsURLKey];
#pragma clang diagnostic pop
                  if ([launched isKindOfClass:[NSURL class]]) {
                    g_launch_url = launched;
                    fandhe_forward_url(launched);
                  }
                }
              }];
  g_active_observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:UIApplicationDidBecomeActiveNotification
                  object:nil
                   queue:[NSOperationQueue mainQueue]
              usingBlock:^(NSNotification *__unused note) {
                g_launch_url = nil;
              }];
}
