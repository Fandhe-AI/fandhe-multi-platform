// Phase 11 の拡充ループ（親 #1003）型サンプル「通知」の iOS 実装（Issue #1005）。
// macOS（#1004、`osascript`）とは別経路: iOS はバンドル化されたプロセスから起動する
// ため `UNUserNotificationCenter` が使える（macOS が使わない理由は `crates/
// fandhe-egui-proxy/src/device/notification.rs` モジュール doc 参照）。
//
// `crates/fandhe-egui-proxy/src/device/ios_notification.rs` から C ABI 経由で
// 呼ばれる。`build.rs` が feature `device` 有効時のみ `cc` コンパイルし、
// UserNotifications をリンクする。
//
// 契約（pull 型。`fandhe_apple_biometric.m` と同型、coding-rust.md の FFI 例外規定）:
//   int32_t fandhe_ios_notification_post(uint64_t request_id, const char *title,
//                                        const char *body)
//     -> 戻り値: 0 = 提示依頼を受理（結果は非同期に fandhe_rust_notification_outcome
//        で届く。ちょうど 1 回）。1 = 同期的な受理失敗（NULL・UTF-8 変換失敗。
//        呼び出し元が Internal エラーへ畳む。この場合 fandhe_rust_notification_outcome
//        は呼ばれない）。title/body は NUL 終端 UTF-8。関数内で即座に NSString へ
//        コピーしてから return するため、呼び出し元は return 後に元の文字列
//        バッファを解放してよい（所有権は移らない）。
//   fandhe_rust_notification_outcome(uint64_t request_id, int32_t code)
//     -> Rust 側（`ios_notification.rs`）が公開する `extern "C"` 関数。`code` の
//        写像は Rust 側 `device/notification.rs::outcome_from_code`（SSOT）に
//        一本化する。本ファイルでの対応（`.m` 側で二重に写像しない）:
//          0 = addNotificationRequest が成功
//          1 = 許可が未決定のまま閉じた（requestAuthorization が granted=NO かつ
//              getNotificationSettings で authorizationStatus == NotDetermined）
//          2 = 許可されていない（Denied、または granted=NO かつ Denied）
//          3 = requestAuthorization 自体がエラーを返した
//          4 = addNotificationRequest がエラーを返した
//
// 権限要求は `UNAuthorizationOptionAlert` のみ（Sound/Badge は求めない、
// device-bridge-contracts.md「7. 対応表・権限」の不要な権限を足さない方針）。
// すでに決定済みなら `requestAuthorizationWithOptions:` はダイアログを出さずに
// 現在の状態を返す契約（Apple ドキュメント）ため、事前の設定確認は省く。
//
// 前面表示（`willPresent`）: `UNUserNotificationCenterDelegate` を実装し、
// Banner + List で表示する（バナー通知が実際に出るかは利用者の OS 設定次第。
// 画面文言は「表示されるかは OS の通知設定次第です」に留める、`Shown` を
// 返さない設計は `notification.rs` モジュール doc 参照）。delegate の設定は
// Apple ドキュメント上は「起動完了前」が要件だが、それは通知タップからの
// アプリ起動（本実装はスコープ外、タップ応答は `didReceiveNotificationResponse`
// を実装しない）に対する要件であり、前面表示には遅れて設定しても効く
// （最初の通知要求時に main queue で 1 回だけ設定する）。delegate プロパティは
// weak のため、static で強参照してプロセスの寿命の間保持する。
//
// スレッド: 公開関数は文字列を即座に `NSString` へコピーしたあと
// `dispatch_async(main)` で本処理へ渡す（`fandhe_apple_biometric.m` と同じ
// 方針）。`UNUserNotificationCenter` の completion ブロックは呼ばれるスレッドが
// 契約上未規定のため、`fandhe_rust_notification_outcome` の呼び出し自体も
// `dispatch_async(main)` でラップし、Rust 側へは常に main queue から届ける
// （呼び出し元 `ios_notification.rs::present` の `@autoreleasepool` 前提と
// 揃える）。各経路で Rust への通知がちょうど 1 回になるよう、分岐ごとに
// 完了パスを 1 本に絞ってある（下記コメント参照）。
//
// 情報非保持: タイトル・本文・エラーの詳細（`NSError` の文言）を一切
// ログへ出力しない（security.md A09）。`NSLog` は使わない。

#import <UserNotifications/UserNotifications.h>

// Rust 側（`ios_notification.rs`）が公開する `extern "C"` 関数。
extern void fandhe_rust_notification_outcome(uint64_t request_id, int32_t code);

// `UNUserNotificationCenter.delegate` は weak プロパティのため、プロセスの
// 寿命の間 delegate を生かし続けるには強参照を static で保持する必要がある
// （doc 冒頭「前面表示」節参照）。
@interface FandheNotificationDelegate : NSObject <UNUserNotificationCenterDelegate>
@end

@implementation FandheNotificationDelegate
// アプリが前面にあるときに届いた通知の表示方法（completionHandler は 1 回だけ
// 呼ぶ、iOS 17.0 以降で有効な `UNNotificationPresentationOptionList`/`Banner`
// を使う。`platforms/apple/project.yml` の deployment target が iOS 17.0 の
// ため `@available` 分岐は不要）。
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
        willPresentNotification:(UNNotification *)notification
          withCompletionHandler:(void (^)(UNNotificationPresentationOptions options))completionHandler {
  completionHandler(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList);
}
@end

static FandheNotificationDelegate *g_notification_delegate = nil;

// `willPresent` の delegate 設定を最初の 1 回だけ main queue 上で行う
// （呼び出し元は常に main queue 上、doc の「スレッド」節参照）。
static void FandheEnsureNotificationDelegate(void) {
  if (g_notification_delegate == nil) {
    g_notification_delegate = [[FandheNotificationDelegate alloc] init];
    [UNUserNotificationCenter currentNotificationCenter].delegate = g_notification_delegate;
  }
}

// 許可済み（登録してよい）とみなす状態。`Authorized`/`Provisional`/`Ephemeral`
// のみを登録してよい状態として扱う（実装計画 §2.2）。
static BOOL FandheAuthorizationStatusAllowsRegistration(UNAuthorizationStatus status) {
  return status == UNAuthorizationStatusAuthorized || status == UNAuthorizationStatusProvisional ||
         status == UNAuthorizationStatusEphemeral;
}

// 許可要求後に `addNotificationRequest:` を試みる（許可済みの場合のこの関数への
// 到達は `requestAuthorization` の completion から）。
static void FandhePostNotificationRequest(uint64_t request_id, NSString *title, NSString *body) {
  UNMutableNotificationContent *content = [[UNMutableNotificationContent alloc] init];
  content.title = title;
  content.body = body;

  UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:[[NSUUID UUID] UUIDString]
                                                                        content:content
                                                                        trigger:nil];
  [[UNUserNotificationCenter currentNotificationCenter]
      addNotificationRequest:request
        withCompletionHandler:^(NSError *_Nullable error) {
          dispatch_async(dispatch_get_main_queue(), ^{
            // 経路: addNotificationRequest の成否のみでちょうど 1 回通知する。
            fandhe_rust_notification_outcome(request_id, error == nil ? 0 : 4);
          });
        }];
}

static void FandhePostNotificationOnMainThread(uint64_t request_id, NSString *title, NSString *body) {
  FandheEnsureNotificationDelegate();

  UNUserNotificationCenter *center = [UNUserNotificationCenter currentNotificationCenter];
  [center requestAuthorizationWithOptions:UNAuthorizationOptionAlert
                         completionHandler:^(BOOL granted, NSError *_Nullable error) {
                           dispatch_async(dispatch_get_main_queue(), ^{
                             if (error != nil) {
                               // 経路: requestAuthorization 自体のエラー。
                               fandhe_rust_notification_outcome(request_id, 3);
                               return;
                             }
                             if (granted) {
                               // 経路: 許可された。登録を試みる（結果は
                               // FandhePostNotificationRequest 側でちょうど 1 回通知）。
                               FandhePostNotificationRequest(request_id, title, body);
                               return;
                             }
                             // granted=NO: 拒否 (Denied) か未決定のまま閉じた
                             // (NotDetermined) かを getNotificationSettings で
                             // 見分ける（実装計画 §2.2）。
                             [center getNotificationSettingsWithCompletionHandler:^(
                                         UNNotificationSettings *_Nonnull settings) {
                               dispatch_async(dispatch_get_main_queue(), ^{
                                 UNAuthorizationStatus status = settings.authorizationStatus;
                                 if (FandheAuthorizationStatusAllowsRegistration(status)) {
                                   // granted=NO だが設定確認時点では許可済みに
                                   // 遷移していた競合（稀）。登録を試みる。
                                   FandhePostNotificationRequest(request_id, title, body);
                                 } else if (status == UNAuthorizationStatusNotDetermined) {
                                   // 経路: 未決定のまま閉じた（キャンセル扱い）。
                                   fandhe_rust_notification_outcome(request_id, 1);
                                 } else {
                                   // 経路: 拒否 (Denied)。
                                   fandhe_rust_notification_outcome(request_id, 2);
                                 }
                               });
                             }];
                           });
                         }];
}

int32_t fandhe_ios_notification_post(uint64_t request_id, const char *title, const char *body) {
  if (title == NULL || body == NULL) {
    return 1;
  }
  NSString *nsTitle = [NSString stringWithUTF8String:title];
  NSString *nsBody = [NSString stringWithUTF8String:body];
  if (nsTitle == nil || nsBody == nil) {
    return 1;
  }

  dispatch_async(dispatch_get_main_queue(), ^{
    FandhePostNotificationOnMainThread(request_id, nsTitle, nsBody);
  });
  return 0;
}
