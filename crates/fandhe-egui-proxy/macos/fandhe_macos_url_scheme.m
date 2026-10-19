// Phase 11 の拡充ループ（親 #1116）型サンプル「カスタム URL scheme」: macOS 実装
// （Issue #1215）。`NSAppleEventManager` の `kAEGetURL` ハンドラで URL を受ける。
// winit 0.30 は自前の application delegate を登録し、差し替えると panic する。この方式は
// delegate に触れないため、その delegate と衝突しない。macOS 専用のため `apple/*.m` ではなく `macos/` に置く
// （`fandhe_macos_background_task.m` と同じ判断）。
//
// `crates/fandhe-egui-proxy/src/device/apple_url_scheme.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_apple_url_scheme_register(void) -> void
//     main thread から呼ぶ。冪等。ハンドラを即時に登録し、さらに
//     `NSApplicationWillFinishLaunchingNotification` でも登録し直す（Apple の推奨は
//     WillFinishLaunching までに登録すること。DidFinishLaunching では起動時の URL に
//     間に合わない）。`setEventHandler:` は上書きのため二重登録は無害。
//     `NSAppleEventManager` はハンドラを retain しないので、ハンドラと observer トークンは
//     static の強参照で持ち続ける（解放しない）。
//   fandhe_rust_url_scheme_received(const uint8_t *ptr, size_t len) -> Rust 側
//     （`apple_url_scheme.rs`）が公開する `extern "C"` 関数。URL を UTF-8 で渡す。
//     文字列が取れなかったときは (NULL, 0)。ポインタは呼び出し中だけ有効で、Rust 側は
//     呼び出しの中で検査して保持する。
//
// スレッド契約: Apple Event は main thread の run loop から届く。ハンドラの中身は
// `@autoreleasepool` で囲む（`device-bridge-contracts.md`「5. FFI 境界」）。
//
// アプリ終了時: プロセスが終わるとハンドラ登録も消えるため、明示的な解放は不要。

#import <AppKit/AppKit.h>
#import <CoreServices/CoreServices.h>
#import <Foundation/Foundation.h>

extern void fandhe_rust_url_scheme_received(const uint8_t *ptr, size_t len);

@interface FandheUrlSchemeHandler : NSObject
- (void)handleGetURLEvent:(NSAppleEventDescriptor *)event
           withReplyEvent:(NSAppleEventDescriptor *)reply;
@end

@implementation FandheUrlSchemeHandler
- (void)handleGetURLEvent:(NSAppleEventDescriptor *)event
           withReplyEvent:(NSAppleEventDescriptor *)reply {
  @autoreleasepool {
    NSString *url = [[event paramDescriptorForKeyword:keyDirectObject] stringValue];
    NSData *data = [url dataUsingEncoding:NSUTF8StringEncoding];
    if (data == nil || data.length == 0) {
      fandhe_rust_url_scheme_received(NULL, 0);
    } else {
      fandhe_rust_url_scheme_received((const uint8_t *)data.bytes, (size_t)data.length);
    }
  }
}
@end

static FandheUrlSchemeHandler *g_handler = nil;
static id g_observer = nil;

static void fandhe_install_handler(void) {
  [[NSAppleEventManager sharedAppleEventManager]
      setEventHandler:g_handler
          andSelector:@selector(handleGetURLEvent:withReplyEvent:)
        forEventClass:kInternetEventClass
           andEventID:kAEGetURL];
}

void fandhe_apple_url_scheme_register(void) {
  if (g_handler != nil) {
    return;
  }
  g_handler = [[FandheUrlSchemeHandler alloc] init];
  fandhe_install_handler();
  g_observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:NSApplicationWillFinishLaunchingNotification
                  object:nil
                   queue:nil
              usingBlock:^(NSNotification *__unused note) {
                fandhe_install_handler();
              }];
}
