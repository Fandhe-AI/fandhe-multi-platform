// 共通層の application delegate（Issue #1325、親 #1159）。`UIApplication.delegate` は 1 枠しかなく、
// winit 0.30.13 の iOS は delegate を持たない（`UIApplicationMain(argc, argv, nil, nil)`）。機能ごとに
// 差し込むと奪い合うため、共通層が 1 つだけ差し込み、出来事を機能の `.m` へ配る。
// 口の宣言は `include/fandhe_ios_app_events.h`。
//
// 受けるのは次の 2 つだけで、UIKit の既定動作は変えない。
//   - `application:openURL:options:`: scene 形態では UIKit が呼ばない旧経路。`ios/fandhe_ios_scene.m` と
//     同じ通知 `FandheSceneOpenURLNotification` へ合流させる。
//   - `application:handleEventsForBackgroundURLSession:completionHandler:`: identifier で登録表を引き、
//     受け手があれば渡す。無ければ completionHandler を即呼ぶ。どちらか一方だけを通るため 1 回だけ呼ばれる。
// `application:continueUserActivity:`・`application:performActionForShortcutItem:` は scene 形態で
// 呼ばれないため足さない（scene delegate が通知へ流す）。
//
// 差し込み時刻: 起動通知は queue:nil で投稿スレッド（main）の同期実行にする。mainQueue 指定だと次の
// run loop へ回り、同じ起動ターンで届く handleEventsForBackgroundURLSession（relaunch）に間に合わない。
// `fandhe-platform-apple/build.rs` が iOS で常にコンパイルするが、リンカは `install` / `register` を
// 参照する機能がリンクされたときだけ本ファイルを成果物へ入れる。
//
// スレッド契約: 登録表は main thread だけが触る。UIKit も delegate を main thread から呼ぶ。
// URL・identifier はログへ出さない（NSLog を使わない）。プロセス終了で消えるため明示的な解放はしない。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

#import "fandhe_ios_app_events.h"

@interface FandheAppDelegate : NSObject <UIApplicationDelegate>
@end

static NSMutableDictionary<NSString *, FandheBackgroundSessionReceiver> *g_receivers = nil;

@implementation FandheAppDelegate
- (BOOL)application:(UIApplication *)app
            openURL:(NSURL *)url
            options:(NSDictionary<UIApplicationOpenURLOptionsKey, id> *)options {
  @autoreleasepool {
    [[NSNotificationCenter defaultCenter] postNotificationName:FandheSceneOpenURLNotification
                                                        object:url];
  }
  return YES;
}

- (void)application:(UIApplication *)application
    handleEventsForBackgroundURLSession:(NSString *)identifier
                      completionHandler:(void (^)(void))completionHandler {
  @autoreleasepool {
    FandheBackgroundSessionReceiver receiver = identifier != nil ? g_receivers[identifier] : nil;
    if (receiver != nil) {
      receiver(completionHandler);
    } else {
      completionHandler();
    }
  }
}
@end

static FandheAppDelegate *g_delegate = nil;
static id g_launch_observer = nil;

void fandhe_ios_app_delegate_install(void) {
  if (g_delegate != nil) {
    return;
  }
  g_delegate = [[FandheAppDelegate alloc] init];
  g_launch_observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:UIApplicationDidFinishLaunchingNotification
                  object:nil
                   queue:nil
              usingBlock:^(NSNotification *note) {
                @autoreleasepool {
                  UIApplication *app = note.object;
                  // ponytail: delegate が既にある場合（将来 winit が持った場合）は上書きせず、
                  // 旧経路の URL・背景転送の完了はつながらない。そうなったら class_addMethod で既存 delegate の
                  // クラスへ両メソッドを足す。
                  if ([app isKindOfClass:[UIApplication class]] && app.delegate == nil) {
                    app.delegate = g_delegate;
                  }
                }
              }];
}

void fandhe_ios_register_background_session(NSString *identifier,
                                            FandheBackgroundSessionReceiver receiver) {
  if (identifier.length == 0 || receiver == nil) {
    return;
  }
  if (g_receivers == nil) {
    g_receivers = [NSMutableDictionary dictionary];
  }
  g_receivers[identifier] = [receiver copy];
  fandhe_ios_app_delegate_install();
}
