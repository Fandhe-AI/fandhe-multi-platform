// Phase 11 の拡充ループ（親 #101）型サンプル「アプリショートカット」: macOS 実装（Issue #424）。
// Dock の右クリックメニューに項目を 1 つ足す。選ばれると固定 URL を url-scheme の受け口
// （`fandhe_rust_url_scheme_received`、`apple_url_scheme.rs`）へ渡す。検査は `UrlInbox` が行う。
//
// winit 0.30.13 は `WinitApplicationDelegate` を `setDelegate` しており（差し替えると panic する）、
// `applicationDockMenu:` は実装していない。そのため delegate のクラスへ `class_addMethod` で
// メソッドを足す。既にメソッドがあれば NO が返るので、上書きせず失敗（-3）として返す。
//
// `crates/fandhe-egui-proxy/src/device/macos_app_shortcuts.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   int32_t fandhe_macos_app_shortcuts_install(title, title_len, url, url_len)
//     main thread から同期で呼ぶ。文字列は UTF-8 で、呼び出しの間だけ有効（ここでコピーする）。
//      0: 登録した（既に登録済みの再呼び出しも 0）
//     -1: main thread 以外、または NSApp か NSApp.delegate が無い
//     -2: 引数が NULL、または文字列を作れない
//     -3: delegate が既に applicationDockMenu: を持つ（上書きしない）
//   未確認: AppKit が Dock 右クリックのたびに respondsToSelector: を見るか（#413 の Mac 目視で確認）。
//
// FFI 規約: 本体を `@autoreleasepool` で囲む。メニューとターゲットはプロセスが生きている間
// static の強参照で持ち、終了時は OS が片付ける。ログは出さない（URL を含みうるため）。

#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#import <objc/runtime.h>
#include <stdint.h>

extern void fandhe_rust_url_scheme_received(const uint8_t *ptr, size_t len);

static NSMenu *g_dock_menu = nil;
static NSString *g_url = nil;

@interface FandheDockMenuTarget : NSObject
- (void)openDemo:(id)sender;
@end

@implementation FandheDockMenuTarget
- (void)openDemo:(id)sender {
  @autoreleasepool {
    NSData *data = [g_url dataUsingEncoding:NSUTF8StringEncoding];
    if (data == nil || data.length == 0) {
      fandhe_rust_url_scheme_received(NULL, 0);
    } else {
      fandhe_rust_url_scheme_received((const uint8_t *)data.bytes, (size_t)data.length);
    }
  }
}
@end

static FandheDockMenuTarget *g_target = nil;

// delegate クラスへ足す実装。登録済みのメニューをそのまま返す。
static NSMenu *fandhe_dock_menu(id __unused self, SEL __unused _cmd,
                                NSApplication *__unused app) {
  return g_dock_menu;
}

int32_t fandhe_macos_app_shortcuts_install(const uint8_t *title, size_t title_len,
                                           const uint8_t *url, size_t url_len) {
  @autoreleasepool {
    if (![NSThread isMainThread] || NSApp == nil || NSApp.delegate == nil) {
      return -1;
    }
    if (title == NULL || url == NULL || title_len == 0 || url_len == 0) {
      return -2;
    }
    if (g_dock_menu != nil) {
      return 0;
    }
    NSString *title_str = [[NSString alloc] initWithBytes:title
                                                   length:title_len
                                                 encoding:NSUTF8StringEncoding];
    NSString *url_str = [[NSString alloc] initWithBytes:url
                                                 length:url_len
                                               encoding:NSUTF8StringEncoding];
    if (title_str == nil || url_str == nil) {
      return -2;
    }
    g_url = url_str;
    g_target = [[FandheDockMenuTarget alloc] init];
    NSMenu *menu = [[NSMenu alloc] initWithTitle:@""];
    NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:title_str
                                                  action:@selector(openDemo:)
                                           keyEquivalent:@""];
    item.target = g_target;
    [menu addItem:item];
    g_dock_menu = menu;
    BOOL added = class_addMethod([NSApp.delegate class], @selector(applicationDockMenu:),
                                 (IMP)fandhe_dock_menu, "@@:@");
    if (!added) {
      g_dock_menu = nil;
      g_target = nil;
      g_url = nil;
      return -3;
    }
    return 0;
  }
}
