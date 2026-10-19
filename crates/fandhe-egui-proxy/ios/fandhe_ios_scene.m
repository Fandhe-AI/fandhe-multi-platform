// iOS の UIScene 化シム（Issue #27）。winit 0.30.13 の iOS は scene 非対応（`UIApplicationMain` に
// delegate を渡さず、`UIWindow initWithFrame:` で `windowScene` 無しの window を作る）だが、
// Apple が外部ディスプレイを契約しているのは scene 経由（`UIWindowSceneSessionRole
// ExternalDisplayNonInteractive`）だけで、`UIScreen.screens` は iOS 16 で非推奨になった。
// 外部画面を列挙する複数ディスプレイ（`fandhe_ios_multi_display.m`）のため、Info.plist に
// `UIApplicationSceneManifest` を宣言し、本ファイルの `FandheSceneDelegate` で scene を受ける。
// TN3187 により scene 未採用のアプリはいずれ起動できなくなるため、scene 化自体も必須の移行。
//
// 役割:
//   - application ロールの scene: winit が作った `windowScene == nil` の window を scene へ付け、
//     表示する（付け直しは冪等。winit の window 作成と scene 接続の順序に依存しない）。
//   - 外部ディスプレイ ロールの scene: window を付けない（ミラーリング・拡張表示は OS の既定のまま）。
//     scene が接続されることで `connectedScenes` から外部画面を列挙できる。
//   - URL: scene 形態では `application:openURL:options:` が呼ばれず scene 側へ届くため、
//     通知 `FandheSceneOpenURLNotification`（object = NSURL）へ流す。受信側は
//     `fandhe_ios_url_scheme.m`（本ファイルは feature `device` に依存しない）。
//   - ホーム画面のショートカット（Issue #425）: 未起動からの起動は `connectionOptions.shortcutItem`、
//     起動中は `windowScene:performActionForShortcutItem:completionHandler:` で届く。type は
//     Info.plist の固定 URL なので、そのまま URL として同じ通知へ流す（検査は Rust 側 `UrlInbox`）。
//     scene 形態では `application:performActionForShortcutItem:` は使わない。
//
// `build.rs` が feature・計測ビルドに関係なく常にコンパイルする（Info.plist は 1 つのため）。
// `src/ios_scene.rs` の `install()` が `fandhe_ios_scene_install()` を呼んで、リンカが
// 本ファイルのクラスを捨てないようにする。winit が scene に対応（upstream #4723）したら削除する。
//
// スレッド契約: UIKit が main thread から呼ぶ。中身は `@autoreleasepool` で囲む。
// URL・画面情報はログへ出さない。

#import <UIKit/UIKit.h>

NSString *const FandheSceneOpenURLNotification = @"FandheSceneOpenURLNotification";

// application ロールの scene を返す（無ければ nil）。
static UIWindowScene *fandhe_app_scene(void) {
  for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
    if ([scene isKindOfClass:[UIWindowScene class]] &&
        [scene.session.role isEqualToString:UIWindowSceneSessionRoleApplication]) {
      return (UIWindowScene *)scene;
    }
  }
  return nil;
}

// application の scene より先に可視化された window の待機表（弱参照。main thread のみ）。
// scene 接続時に付け直す（window 先着・scene 先着のどちらの順序でも接続が完了する）。
static NSHashTable<UIWindow *> *fandhe_pending_windows(void) {
  static NSHashTable<UIWindow *> *t;
  static dispatch_once_t once;
  dispatch_once(&once, ^{
    t = [NSHashTable weakObjectsHashTable];
  });
  return t;
}

static void fandhe_bind_window(UIWindow *w, UIWindowScene *scene) {
  [fandhe_pending_windows() removeObject:w];
  w.windowScene = scene;
  [w makeKeyAndVisible];
}

// scene 未接続の window を application の scene へ付けて表示する（冪等）。
// scene がまだ無ければ待機表へ入れ、`willConnectToSession` で付ける。
static void fandhe_attach_window(UIWindow *w) {
  if (w.windowScene != nil) {
    return;
  }
  UIWindowScene *scene = fandhe_app_scene();
  if (scene != nil) {
    fandhe_bind_window(w, scene);
  } else {
    [fandhe_pending_windows() addObject:w];
  }
}

static void fandhe_post_urls(NSSet<UIOpenURLContext *> *contexts) {
  for (UIOpenURLContext *c in contexts) {
    [[NSNotificationCenter defaultCenter] postNotificationName:FandheSceneOpenURLNotification
                                                        object:c.URL];
  }
}

// ショートカットの type を URL として通知へ流す。URL にできなければ何も送らず NO。
static BOOL fandhe_post_shortcut(UIApplicationShortcutItem *item) {
  NSURL *url = [NSURL URLWithString:item.type];
  if (url == nil) {
    return NO;
  }
  [[NSNotificationCenter defaultCenter] postNotificationName:FandheSceneOpenURLNotification
                                                      object:url];
  return YES;
}

@interface FandheSceneDelegate : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation FandheSceneDelegate
- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)options {
  @autoreleasepool {
    if (![scene isKindOfClass:[UIWindowScene class]] ||
        [session.role isEqualToString:UIWindowSceneSessionRoleExternalDisplayNonInteractive]) {
      return;
    }
    for (UIWindow *w in fandhe_pending_windows().allObjects) {
      if (w.windowScene == nil) {
        fandhe_bind_window(w, (UIWindowScene *)scene);
      } else {
        [fandhe_pending_windows() removeObject:w];
      }
    }
    fandhe_post_urls(options.URLContexts);
    // 未起動からの起動では performActionForShortcutItem は呼ばれずここにだけ入るため、二重には届かない。
    if (options.shortcutItem != nil) {
      fandhe_post_shortcut(options.shortcutItem);
    }
  }
}

// 起動中にショートカットが選ばれた。completion は必ず 1 回だけ呼ぶ。
- (void)windowScene:(UIWindowScene *)windowScene
    performActionForShortcutItem:(UIApplicationShortcutItem *)shortcutItem
               completionHandler:(void (^)(BOOL succeeded))completionHandler {
  BOOL ok = NO;
  @autoreleasepool {
    ok = fandhe_post_shortcut(shortcutItem);
  }
  if (completionHandler != nil) {
    completionHandler(ok);
  }
}

- (void)scene:(UIScene *)scene openURLContexts:(NSSet<UIOpenURLContext *> *)URLContexts {
  @autoreleasepool {
    fandhe_post_urls(URLContexts);
  }
}
@end

// リンク用アンカー（Rust から参照してクラスの dead strip を防ぐ）。
// winit は scene 接続より後（`resumed`）に window を作って `makeKeyAndVisible` するため、window が
// 可視・key になった通知で scene 未接続なら付け直す。観測は 1 回だけ登録する。
void fandhe_ios_scene_install(void) {
  static dispatch_once_t once;
  dispatch_once(&once, ^{
    (void)[FandheSceneDelegate class];
    void (^handler)(NSNotification *) = ^(NSNotification *note) {
      @autoreleasepool {
        if ([note.object isKindOfClass:[UIWindow class]]) {
          fandhe_attach_window(note.object);
        }
      }
    };
    for (NSNotificationName name in @[ UIWindowDidBecomeVisibleNotification,
                                       UIWindowDidBecomeKeyNotification ]) {
      [[NSNotificationCenter defaultCenter] addObserverForName:name
                                                        object:nil
                                                         queue:[NSOperationQueue mainQueue]
                                                    usingBlock:handler];
    }
  });
}
