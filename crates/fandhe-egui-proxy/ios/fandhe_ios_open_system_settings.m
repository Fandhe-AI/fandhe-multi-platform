// 「OS の設定画面を開く」（親 #171、Issue #799）の iOS 実装。
// `UIApplication.openURL:options:completionHandler:` で自アプリの設定頁・通知の設定頁を開く。
//
// `crates/fandhe-egui-proxy/src/device/ios_open_system_settings.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS 分岐の feature `device` 有効時のみ `cc` コンパイルする（UIKit は既にリンク済み）。
//
// 契約（pull 型。`fandhe_ios_app_badge.m` と同型）:
//   int32_t fandhe_ios_open_system_settings_open(uint64_t request_id, int32_t page)
//     page: 0 = 自アプリの設定、1 = 通知の設定（写像の SSOT は Rust 側 `page_code`。ここでは重複定義しない）。
//     -> 戻り値 0 = 受理（結果は fandhe_rust_open_system_settings_outcome にちょうど 1 回届く）。
//        未知の page は同期で非 0 を返し、結果コールバックは呼ばない。
//   fandhe_rust_open_system_settings_outcome(uint64_t request_id, int32_t code)
//     -> Rust 側が公開する `extern "C"` 関数。code の写像は Rust 側 `outcome_from_ios_code`（SSOT）:
//          0 = completion の success == YES（OS が URL を受け付けた）
//          1 = completion の success == NO（開けなかった）
//          2 = 依頼自体ができなかった（URL を作れない・sharedApplication が無い・OS 版が足りない）
//
// 方針: `canOpenURL` と非公開の URL（`App-Prefs:` 等）は使わない。公開定数のみ。期限・タイマーは持たない。
// 開けなかった頁を別の頁で代替しない。
//
// スレッド: 公開関数は page を検証して main queue へ渡し即 return する。`openURL` は main で呼び、
// Rust への通知は main queue 上の 1 本の完了パス（FandheOpenSettingsFinish）に絞る。
//
// 情報非保持: URL・エラーをログへ出さない（security.md A09）。NSLog は使わない。
// 登録トークン・オブザーバーは持たない。

#import <UIKit/UIKit.h>

extern void fandhe_rust_open_system_settings_outcome(uint64_t request_id, int32_t code);

// Rust へ main queue 上から結果を 1 回だけ届ける。
static void FandheOpenSettingsFinish(uint64_t request_id, int32_t code) {
  dispatch_async(dispatch_get_main_queue(), ^{
    fandhe_rust_open_system_settings_outcome(request_id, code);
  });
}

// 頁コードから公開定数の URL を作る。作れなければ nil。
static NSURL *FandheOpenSettingsURL(int32_t page) {
  NSString *string = nil;
  if (page == 0) {
    string = UIApplicationOpenSettingsURLString;
  } else if (page == 1) {
    if (@available(iOS 16.0, *)) {
      string = UIApplicationOpenNotificationSettingsURLString;
    }
  }
  return string != nil ? [NSURL URLWithString:string] : nil;
}

static void FandheOpenSettingsOnMain(uint64_t request_id, int32_t page) {
  NSURL *url = FandheOpenSettingsURL(page);
  UIApplication *app = UIApplication.sharedApplication;
  if (url == nil || app == nil) {
    // 経路: 依頼自体ができなかった。
    FandheOpenSettingsFinish(request_id, 2);
    return;
  }
  [app openURL:url
        options:@{}
        completionHandler:^(BOOL success) {
          // 経路: OS の成否をそのまま写す。
          FandheOpenSettingsFinish(request_id, success ? 0 : 1);
        }];
}

int32_t fandhe_ios_open_system_settings_open(uint64_t request_id, int32_t page) {
  if (page != 0 && page != 1) {
    return 1;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    @autoreleasepool {
      FandheOpenSettingsOnMain(request_id, page);
    }
  });
  return 0;
}
