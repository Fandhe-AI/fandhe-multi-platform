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
// `ios/fandhe_ios_scene.m` の scene delegate が受けた URL（`connectionOptions.URLContexts` と
// `scene:openURLContexts:`）を通知 `FandheSceneOpenURLNotification` で投げ、ここで受ける。
// (2) 旧来経路: winit 0.30.13 の iOS は application delegate を持たず
// （`UIApplicationMain(argc, argv, nil, nil)`）、scene 形態でない場合の `application:openURL:options:`
// を受けるために自前の delegate を差し込む（scene 形態では呼ばれないが害は無い）。
// `UIApplicationMain` より前は `sharedApplication` が無いので、最も早い
// `UIApplicationDidFinishLaunchingNotification` で `delegate == nil` のときだけ差し込む。
// 実装するのは `application:openURL:options:` と、背景ダウンロードの
// `application:handleEventsForBackgroundURLSession:completionHandler:`（Issue #391。`UIApplication.delegate` は
// 1 枠しかなく、UIKit は setDelegate 時に応答メソッドを記録するため、別の delegate を差さず本クラスへ足す。
// 転送先は `apple/fandhe_apple_background_download.m`）だけで、UIKit の既定動作は変えない。
//
// 契約:
//   fandhe_apple_url_scheme_register(void) -> void
//     main thread から `UIApplicationMain` より前に呼ぶ。冪等。起動通知の observer と
//     delegate は static の強参照で持ち続ける（`UIApplication.delegate` は weak のため）。
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

extern void fandhe_rust_url_scheme_received(const uint8_t *ptr, size_t len);
extern void fandhe_apple_bgdl_handle_events(NSString *identifier, void (^handler)(void));

static void fandhe_forward_url(NSURL *url) {
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

@interface FandheUrlSchemeAppDelegate : NSObject <UIApplicationDelegate>
@end

@implementation FandheUrlSchemeAppDelegate
- (BOOL)application:(UIApplication *)app
            openURL:(NSURL *)url
            options:(NSDictionary<UIApplicationOpenURLOptionsKey, id> *)options {
  @autoreleasepool {
    fandhe_handle_open_url(url);
  }
  return YES;
}

// OS が背景ダウンロードの完了でアプリを起こした。handler は `.m` が 1 回だけ呼ぶ。
- (void)application:(UIApplication *)application
    handleEventsForBackgroundURLSession:(NSString *)identifier
                      completionHandler:(void (^)(void))completionHandler {
  @autoreleasepool {
    fandhe_apple_bgdl_handle_events(identifier, completionHandler);
  }
}
@end

static FandheUrlSchemeAppDelegate *g_delegate = nil;
static id g_launch_observer = nil;
static id g_active_observer = nil;
static id g_scene_observer = nil;

void fandhe_apple_url_scheme_register(void) {
  if (g_delegate != nil) {
    return;
  }
  g_delegate = [[FandheUrlSchemeAppDelegate alloc] init];
  g_scene_observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:@"FandheSceneOpenURLNotification"
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
                   // queue:nil は投稿スレッド（main）で同期実行される。mainQueue 指定だと次の run loop へ
                   // 回り、同じ起動ターンで届く handleEventsForBackgroundURLSession（relaunch）に
                   // delegate が間に合わない（Issue #391）。
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
                  UIApplication *app = note.object;
                  // ponytail: delegate が既にある場合（将来 winit が持った場合）は上書きせず、
                  // ウォーム経路はつながらない。そうなったら class_addMethod で既存 delegate の
                  // クラスへ application:openURL:options: を足す。
                  if ([app isKindOfClass:[UIApplication class]] && app.delegate == nil) {
                    app.delegate = g_delegate;
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
