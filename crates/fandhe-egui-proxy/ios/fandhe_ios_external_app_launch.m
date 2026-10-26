// 「電話発信・地図など他アプリの起動」（親 #176、Issue #862）の iOS 実装。
// `UIApplication.openURL:options:completionHandler:` で固定の URL（`tel:` / Apple 地図の統一 URL）を開く。
//
// `crates/fandhe-egui-proxy/src/device/ios_external_app_launch.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS 分岐の feature `device` 有効時のみ `cc` コンパイルする（UIKit は既にリンク済み）。
//
// 契約（pull 型。`fandhe_ios_open_system_settings.m` と同型）:
//   int32_t fandhe_ios_external_app_launch_open(uint64_t request_id, const char *url)
//     url: NUL 終端の UTF-8。呼び出し中だけ読み、保持しない（Rust 側の固定定数のみが渡る）。
//     -> 戻り値 0 = 受理（結果は fandhe_rust_external_app_launch_outcome にちょうど 1 回届く）。
//        url が NULL・NSURL を作れない場合は同期で非 0 を返し、結果コールバックは呼ばない。
//   fandhe_rust_external_app_launch_outcome(uint64_t request_id, int32_t code)
//     -> Rust 側が公開する `extern "C"` 関数。code の写像は Rust 側 `outcome_from_ios_code`（SSOT）:
//          0 = completion の success == YES（OS が URL を受け付けた）
//          1 = completion の success == NO（開けるアプリなし）
//          2 = 依頼自体ができなかった（sharedApplication が無い）
//
// 方針: `canOpenURL` は使わない（他アプリの存在を探る行為になる）。期限・タイマーは持たない。
//
// スレッド: NSURL 化は呼び出し元スレッドで同期に済ませ、`openURL` は main で呼ぶ。
// Rust への通知は main queue 上の 1 本の完了パス（FandheExternalLaunchFinish）に絞る。
//
// 情報非保持: URL・エラーをログへ出さない（security.md A09）。NSLog は使わない。

#import <UIKit/UIKit.h>

extern void fandhe_rust_external_app_launch_outcome(uint64_t request_id, int32_t code);

// Rust へ main queue 上から結果を 1 回だけ届ける。
static void FandheExternalLaunchFinish(uint64_t request_id, int32_t code) {
  dispatch_async(dispatch_get_main_queue(), ^{
    fandhe_rust_external_app_launch_outcome(request_id, code);
  });
}

static void FandheExternalLaunchOnMain(uint64_t request_id, NSURL *url) {
  UIApplication *app = UIApplication.sharedApplication;
  if (app == nil) {
    // 経路: 依頼自体ができなかった。
    FandheExternalLaunchFinish(request_id, 2);
    return;
  }
  [app openURL:url
        options:@{}
        completionHandler:^(BOOL success) {
          // 経路: OS の成否をそのまま写す。NO を成功に畳まない。
          FandheExternalLaunchFinish(request_id, success ? 0 : 1);
        }];
}

int32_t fandhe_ios_external_app_launch_open(uint64_t request_id, const char *url) {
  if (url == NULL) {
    return 1;
  }
  NSString *string = [NSString stringWithUTF8String:url];
  NSURL *nsurl = string != nil ? [NSURL URLWithString:string] : nil;
  if (nsurl == nil) {
    return 1;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    @autoreleasepool {
      FandheExternalLaunchOnMain(request_id, nsurl);
    }
  });
  return 0;
}
