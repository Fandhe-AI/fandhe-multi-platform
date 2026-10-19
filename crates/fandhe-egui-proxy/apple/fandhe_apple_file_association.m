// Phase 11 の拡充ループ（親 #102）型サンプル「ファイルの関連付け」: macOS 実装（Issue #416）。
// `NSAppleEventManager` の `kAEOpenDocuments`（odoc）ハンドラでファイルを受ける。
// winit の application delegate は差し替えず、url-scheme（`macos/fandhe_macos_url_scheme.m`）と
// 同じ Apple Event 方式にする。macOS・iOS で受け口（Rust 側 `apple_file_association.rs`）を共用
// するため `apple/` に置く。iOS 側（`TARGET_OS_IOS`）は b-ios が同じ C ABI 関数を呼ぶ形で書く。
//
// `crates/fandhe-egui-proxy/src/device/apple_file_association.rs` から C ABI 経由で呼ばれる。
// `build.rs` が feature `device` 有効時にコンパイルする。
//
// 契約:
//   fandhe_apple_file_association_register(void) -> void   （macOS のみ）
//     main thread から呼ぶ。冪等。ハンドラを即時に登録し、さらに
//     `NSApplicationWillFinishLaunchingNotification` でも登録し直す。NSApplication は
//     WillFinishLaunching で自前の odoc ハンドラを入れるため、2 回目でそれを上書きして
//     起動時のファイルを受ける。ハンドラと observer は static の強参照で持ち続ける
//     （`NSAppleEventManager` はハンドラを retain しない）。
//   fandhe_rust_file_association_received(const uint8_t *ptr, size_t len) -> Rust 側が公開する関数
//     ファイルパス（`fileSystemRepresentation`、NUL なしの len バイト）を渡す。パスが取れない
//     要素は (NULL, 0)（Rust 側で拒否として数える）。ポインタは呼び出し中だけ有効で、
//     Rust 側は呼び出しの中で検査して読み切る。
//
// スレッド契約: Apple Event は main thread の run loop から届く。ハンドラの中身は
// `@autoreleasepool` で囲む（`device-bridge-contracts.md`「5. FFI 境界」）。
// ponytail: 読み取りは main thread で同期に行う（上限 8 MiB × 件数）。遅いボリュームで固まる
// ことは守らない範囲。専用スレッドへ移す道はあるが、直列キューの順序保証が要る。
//
// アプリ終了時: プロセスが終わるとハンドラ登録も消えるため、明示的な解放は不要。

#import <TargetConditionals.h>
#import <Foundation/Foundation.h>

#if TARGET_OS_OSX

#import <AppKit/AppKit.h>
#import <CoreServices/CoreServices.h>

extern void fandhe_rust_file_association_received(const uint8_t *ptr, size_t len);

@interface FandheFileAssociationHandler : NSObject
- (void)handleOpenDocumentsEvent:(NSAppleEventDescriptor *)event
                  withReplyEvent:(NSAppleEventDescriptor *)reply;
@end

@implementation FandheFileAssociationHandler

// 1 要素を Rust へ渡す。ファイル URL でない・パスが取れない要素は (NULL, 0)。
static void fandhe_deliver(NSAppleEventDescriptor *item) {
  NSURL *url = [item fileURLValue];
  const char *path = (url != nil && url.isFileURL) ? url.fileSystemRepresentation : NULL;
  if (path == NULL || path[0] == '\0') {
    fandhe_rust_file_association_received(NULL, 0);
  } else {
    fandhe_rust_file_association_received((const uint8_t *)path, strlen(path));
  }
}

- (void)handleOpenDocumentsEvent:(NSAppleEventDescriptor *)event
                  withReplyEvent:(NSAppleEventDescriptor *)reply {
  @autoreleasepool {
    NSAppleEventDescriptor *direct = [event paramDescriptorForKeyword:keyDirectObject];
    if (direct == nil) {
      fandhe_rust_file_association_received(NULL, 0);
      return;
    }
    if (direct.descriptorType == typeAEList) {
      for (NSInteger i = 1; i <= direct.numberOfItems; i++) {
        fandhe_deliver([direct descriptorAtIndex:i]);
      }
    } else {
      fandhe_deliver(direct);
    }
  }
}
@end

static FandheFileAssociationHandler *g_handler = nil;
static id g_observer = nil;

static void fandhe_install_handler(void) {
  [[NSAppleEventManager sharedAppleEventManager]
      setEventHandler:g_handler
          andSelector:@selector(handleOpenDocumentsEvent:withReplyEvent:)
        forEventClass:kCoreEventClass
           andEventID:kAEOpenDocuments];
}

void fandhe_apple_file_association_register(void) {
  if (g_handler != nil) {
    return;
  }
  g_handler = [[FandheFileAssociationHandler alloc] init];
  fandhe_install_handler();
  g_observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:NSApplicationWillFinishLaunchingNotification
                  object:nil
                   queue:nil
              usingBlock:^(NSNotification *__unused note) {
                fandhe_install_handler();
              }];
}

#elif TARGET_OS_IOS

// iOS は b-ios（親 #102）が書く。delegate の `application:openURL:options:` と起動時の
// `launchOptions` から、ファイル URL のパスを `fandhe_rust_file_association_received` へ渡す。
// このファイルを iOS でもコンパイルしているのは、b-ios が build.rs を触らずに済むため。

#endif
