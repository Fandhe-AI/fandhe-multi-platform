// Phase 11 の拡充ループ（親 #122）型サンプル「共有の受け取り」: macOS 実装（Issue #606）。
// Info.plist の `NSServices`（サービスメニュー）と `NSApp.servicesProvider` でテキストを受ける。
// Share Extension は使わない（拡張ターゲット・entitlement が要るため）。
// 参考: Apple「System Services」の providing / properties
//   https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/SysServices/introduction.html
// `NSServices` は LaunchServices に登録された .app でしか効かないため、`scripts/apple/
// bundle-macos-url-scheme.sh` が .app 化の際に足す（素の実行体にはサービスが出ない）。
//
// `crates/fandhe-egui-proxy/src/device/macos_share_receive.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_share_receive_register(void) -> void
//     main thread から `run_native` より前に呼ぶ。冪等。provider を作って static の強参照で
//     持ち、`NSApplicationWillFinishLaunchingNotification` の中で
//     `[NSApp setServicesProvider:]` を呼ぶ（observer トークンも static で持つ）。
//     register 時点（`run_native` 前）では `NSApp` / `[NSApplication sharedApplication]` に
//     触れない。winit 0.30 は自前の NSApplication サブクラスと delegate を使うため、先に生成・
//     差し替えると壊れる。delegate には触れない。`servicesProvider` が provider を retain するかに
//     依らず、static の強参照で保持するため問題ない。
//   fandhe_rust_share_receive_received(const uint8_t *ptr, size_t len) -> int32_t
//     Rust 側（`macos_share_receive.rs`）が公開する `extern "C"` 関数。テキストを UTF-8 で
//     渡す。文字列が取れなかったときは (NULL, 0)。ポインタは呼び出し中だけ有効で、Rust 側は
//     呼び出しの中で検査・コピーまで完了する。戻り値は 0 = 受理、1 = 拒否。
//
// 本文は NUL を保つため `UTF8String` ではなく `dataUsingEncoding:` で bytes を渡し、制御文字の
// 検査は Rust 側で行う。取れなかった結果を成功に畳まない（拒否として数える）。
// サービスへ返すエラー文字列は本文を含まない固定文言。本文はログへ出さない。
//
// スレッド契約: サービス要求は main thread の run loop から届く。ハンドラの中身は
// `@autoreleasepool` で囲む（`device-bridge-contracts.md`「5. FFI 境界」）。
//
// アプリ終了時: プロセスが終わると provider 登録も消えるため、明示的な解放は不要。

#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>

extern int32_t fandhe_rust_share_receive_received(const uint8_t *ptr, size_t len);

@interface FandheShareReceiveProvider : NSObject
// セレクタ名は Info.plist の `NSMessage = receiveSharedText` と対（bundle-macos-url-scheme.sh）。
- (void)receiveSharedText:(NSPasteboard *)pboard
                 userData:(NSString *)userData
                    error:(NSString *_Nullable *_Nullable)error;
@end

@implementation FandheShareReceiveProvider
- (void)receiveSharedText:(NSPasteboard *)pboard
                 userData:(NSString *)userData
                    error:(NSString *_Nullable *_Nullable)error {
  @autoreleasepool {
    NSString *text = [pboard stringForType:NSPasteboardTypeString];
    NSData *data = [text dataUsingEncoding:NSUTF8StringEncoding];
    int32_t rc;
    if (data == nil || data.length == 0) {
      rc = fandhe_rust_share_receive_received(NULL, 0);
    } else {
      rc = fandhe_rust_share_receive_received((const uint8_t *)data.bytes, (size_t)data.length);
    }
    if (rc != 0 && error != NULL) {
      *error = @"受け取れませんでした";
    }
  }
}
@end

static FandheShareReceiveProvider *g_provider = nil;
static id g_observer = nil;

void fandhe_macos_share_receive_register(void) {
  if (g_provider != nil) {
    return;
  }
  g_provider = [[FandheShareReceiveProvider alloc] init];
  g_observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:NSApplicationWillFinishLaunchingNotification
                  object:nil
                   queue:nil
              usingBlock:^(NSNotification *__unused note) {
                [NSApp setServicesProvider:g_provider];
                NSUpdateDynamicServices();
              }];
}
