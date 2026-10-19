// Phase 11 の拡充ループ（親 #102）型サンプル「ファイルの関連付け」: macOS 実装（Issue #416）。
// `NSAppleEventManager` の `kAEOpenDocuments`（odoc）ハンドラでファイルを受ける。
// winit の application delegate は差し替えず、url-scheme（`macos/fandhe_macos_url_scheme.m`）と
// 同じ Apple Event 方式にする。macOS・iOS で受け口（Rust 側 `apple_file_association.rs`）を共用
// するため `apple/` に置く。iOS 側（`TARGET_OS_IOS`、Issue #417）は scene の通知から同じ C ABI 関数を呼ぶ。
//
// `crates/fandhe-egui-proxy/src/device/apple_file_association.rs` から C ABI 経由で呼ばれる。
// `build.rs` が feature `device` 有効時にコンパイルする。
//
// 契約:
//   fandhe_apple_file_association_register(void) -> void   （macOS・iOS）
//     iOS は scene 通知の observer を static の強参照で登録する（冪等、main thread、
//     `UIApplicationMain` より前）。以下は macOS: main thread から呼ぶ。冪等。ハンドラを即時に登録し、さらに
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

#import <UIKit/UIKit.h>

// iOS 実装（Issue #417、親 #102 の b-ios）。scene 形態（Issue #27）では
// `application:openURL:options:` が呼ばれないため、`ios/fandhe_ios_scene.m` が投げる通知
// `FandheSceneOpenURLNotification`（object = NSURL、main thread）を購読し、ファイル URL だけを
// 受ける。URL scheme の URL は `ios/fandhe_ios_url_scheme.m` が受け、ファイル URL はそちらで除外する。
// `LSSupportsOpeningDocumentsInPlace=false` のため OS は `Documents/Inbox/` へコピーして渡す。
// Rust 側が呼び出しの中で読み切った後、そのコピーを受理・拒否・失敗を問わず削除する
// （Inbox 直下の通常ファイルだけ。ディレクトリ・シンボリックリンク・それ以外は消さない）。パス・名前はログへ出さない。

extern void fandhe_rust_file_association_received(const uint8_t *ptr, size_t len);

static id g_file_observer = nil;

// 解決済みの親ディレクトリが自アプリの `Documents/Inbox` と一致するときだけ削除する。
static void fandhe_remove_if_in_inbox(NSURL *url) {
  NSURL *docs = [[NSFileManager defaultManager] URLsForDirectory:NSDocumentDirectory
                                                       inDomains:NSUserDomainMask].firstObject;
  if (docs == nil) {
    return;
  }
  NSString *inbox = [[docs URLByAppendingPathComponent:@"Inbox"] URLByResolvingSymlinksInPath].path;
  NSString *parent = [[url URLByDeletingLastPathComponent] URLByResolvingSymlinksInPath].path;
  if (inbox == nil || ![inbox isEqualToString:parent]) {
    return;
  }
  // リンクを追わず種類を見る。通常ファイル以外（ディレクトリ・シンボリックリンク等）は
  // 中身ごと消えるため削除しない。
  NSNumber *regular = nil;
  NSNumber *symlink = nil;
  if (![url getResourceValue:&regular forKey:NSURLIsRegularFileKey error:nil] ||
      ![url getResourceValue:&symlink forKey:NSURLIsSymbolicLinkKey error:nil]) {
    return;
  }
  if (regular.boolValue && !symlink.boolValue) {
    [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
  }
}

void fandhe_apple_file_association_register(void) {
  if (g_file_observer != nil) {
    return;
  }
  g_file_observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:@"FandheSceneOpenURLNotification"
                  object:nil
                   queue:nil
              usingBlock:^(NSNotification *note) {
                @autoreleasepool {
                  NSURL *url = note.object;
                  if (![url isKindOfClass:[NSURL class]] || !url.isFileURL) {
                    return;
                  }
                  const char *path = url.fileSystemRepresentation;
                  if (path == NULL || path[0] == '\0') {
                    fandhe_rust_file_association_received(NULL, 0);
                  } else {
                    fandhe_rust_file_association_received((const uint8_t *)path, strlen(path));
                  }
                  fandhe_remove_if_in_inbox(url);
                }
              }];
}

#endif
