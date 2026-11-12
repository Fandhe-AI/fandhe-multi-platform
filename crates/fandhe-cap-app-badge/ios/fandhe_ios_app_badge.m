// Phase 11 の拡充ループ（親 #1141）型サンプル「バッジの設定」の iOS 実装（Issue #1361）。
// `UNUserNotificationCenter.setBadgeCount(_:withCompletionHandler:)`（iOS 16+）を呼ぶ。
//
// `crates/fandhe-egui-proxy/src/device/ios_app_badge.rs` から C ABI 経由で呼ばれる。
// `build.rs` が feature `device` 有効時のみ `cc` コンパイルする（UserNotifications は
// 通知機能 #1005 で既にリンク済み）。
//
// 契約（pull 型。`fandhe_ios_notification.m` と同型、coding-rust.md の FFI 例外規定）:
//   int32_t fandhe_ios_app_badge_set(uint64_t request_id, uint32_t value)
//     -> 戻り値: 0 = 受理（結果は非同期に fandhe_rust_app_badge_outcome で届く。ちょうど 1 回）。
//        同期で失敗する経路は無い。value は Rust 側 `BadgeValue` で 0〜9999 を検証済み。
//   fandhe_rust_app_badge_outcome(uint64_t request_id, uint32_t value, int32_t code)
//     -> Rust 側（`ios_app_badge.rs`）が公開する `extern "C"` 関数。value は 0 の成功を
//        「消去」表示に分けるため往復させる。code の写像は Rust 側 `outcome_from_code`
//        （SSOT）に一本化し、本ファイルでは写像しない:
//          0 = setBadgeCount が成功
//          1 = 許可が未決定のまま閉じた（NotDetermined のまま）
//          2 = 許可なし（Denied、または許可済みでも badgeSetting が有効でない）
//          3 = requestAuthorization 自体がエラーを返した
//          4 = setBadgeCount がエラーを返した
//          5 = iOS 16 未満（setBadgeCount が無い）
//
// 流れ: value == 0 は消去なので許可要求をせず直接 setBadgeCount:0（ダイアログを出さない）。
// value >= 1 は `UNAuthorizationOptionBadge` のみ要求（Alert/Sound は求めない）し、
// `granted` ではなく getNotificationSettings の authorizationStatus と badgeSetting を見て
// から設定する（通知機能で .alert だけ先に許可済みだと再要求でダイアログが出ず、granted が
// YES でもバッジが無効なことがあるため。嘘の成功を避ける）。
//
// スレッド: 公開関数は `dispatch_async(main)` で本処理へ渡す。completion の呼ばれるスレッドは
// 未規定なので、Rust への通知は常に main queue へ載せ替える。各分岐で Rust への通知が
// ちょうど 1 回になるよう、完了パスを 1 本に絞ってある。
//
// 情報非保持: NSError の文言・値をログへ出力しない（security.md A09）。NSLog は使わない。

#import <UserNotifications/UserNotifications.h>

extern void fandhe_rust_app_badge_outcome(uint64_t request_id, uint32_t value, int32_t code);

// Rust へ main queue から結果を 1 回だけ届ける。
static void FandheAppBadgeFinish(uint64_t request_id, uint32_t value, int32_t code) {
  dispatch_async(dispatch_get_main_queue(), ^{
    fandhe_rust_app_badge_outcome(request_id, value, code);
  });
}

// setBadgeCount の呼び出し（結果はここでちょうど 1 回通知する）。
static void FandheAppBadgeApply(uint64_t request_id, uint32_t value) {
  if (@available(iOS 16.0, *)) {
    [[UNUserNotificationCenter currentNotificationCenter]
        setBadgeCount:(NSInteger)value
        withCompletionHandler:^(NSError *_Nullable error) {
          // 経路: setBadgeCount の成否のみで通知する。
          FandheAppBadgeFinish(request_id, value, error == nil ? 0 : 4);
        }];
  } else {
    // 経路: iOS 16 未満。
    FandheAppBadgeFinish(request_id, value, 5);
  }
}

static BOOL FandheAuthorizationAllows(UNAuthorizationStatus status) {
  return status == UNAuthorizationStatusAuthorized || status == UNAuthorizationStatusProvisional ||
         status == UNAuthorizationStatusEphemeral;
}

static void FandheAppBadgeOnMain(uint64_t request_id, uint32_t value) {
  if (value == 0) {
    // 消去は許可要求をしない（契約: 0 でダイアログを出さない）。
    FandheAppBadgeApply(request_id, value);
    return;
  }
  UNUserNotificationCenter *center = [UNUserNotificationCenter currentNotificationCenter];
  [center requestAuthorizationWithOptions:UNAuthorizationOptionBadge
                         completionHandler:^(BOOL __unused granted, NSError *_Nullable error) {
                           if (error != nil) {
                             // 経路: requestAuthorization 自体のエラー。
                             FandheAppBadgeFinish(request_id, value, 3);
                             return;
                           }
                           // granted だけを信じず、現在の設定で見分ける。
                           [center getNotificationSettingsWithCompletionHandler:^(
                                       UNNotificationSettings *_Nonnull settings) {
                             UNAuthorizationStatus status = settings.authorizationStatus;
                             if (FandheAuthorizationAllows(status) &&
                                 settings.badgeSetting == UNNotificationSettingEnabled) {
                               // 経路: 許可済み。結果は FandheAppBadgeApply 側で通知する。
                               FandheAppBadgeApply(request_id, value);
                             } else if (status == UNAuthorizationStatusNotDetermined) {
                               // 経路: 未決定のまま閉じた（キャンセル扱い）。
                               FandheAppBadgeFinish(request_id, value, 1);
                             } else {
                               // 経路: 拒否、または許可済みでもバッジが無効。
                               FandheAppBadgeFinish(request_id, value, 2);
                             }
                           }];
                         }];
}

int32_t fandhe_ios_app_badge_set(uint64_t request_id, uint32_t value) {
  dispatch_async(dispatch_get_main_queue(), ^{
    FandheAppBadgeOnMain(request_id, value);
  });
  return 0;
}
