// Phase 11 の拡充ループ（親 #134）型サンプル「ログイン時の自動起動」: macOS 実装（Issue #696）。
// `SMAppService.mainApp` でこのアプリ自身をログイン項目に登録・解除し、状態を読み戻す。
// 権限・entitlement は不要（利用者がシステム設定のログイン項目で許可・無効化できる）。
//
// `crates/fandhe-egui-proxy/src/device/macos_launch_at_login.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし、ServiceManagement を
// リンクする。
//
// 契約:
//   呼び出しスレッド: Rust のワーカースレッドから同期で呼ぶ。UI は持たない。
//   戻りコード（4 関数共通）:
//     0 = 成功 / 1 = macOS 13 未満 / 2 = .app 外の素の実行体
//     3 = OS・利用者が拒否 / 4 = 署名不正 / 5 = その他の OS エラー
//   out_status（status / enable / disable、戻りコード 0 のときだけ書く）:
//     0 = notRegistered / 1 = enabled / 2 = requiresApproval / 3 = notFound
//     （SMAppServiceStatus の生値。未知の値は Rust 側が Unknown に倒す）
//   status:        現在の状態を読む。
//   enable:        register。既に登録済み・承認待ちはエラーでも成功扱いにし、読み戻す。
//   disable:       先に status を読み、notRegistered なら OS を呼ばず 0 / 0 を返す（べき等）。
//                  それ以外は unregister。失敗しても読み戻して notRegistered なら 0（べき等）。
//   open_settings: システム設定のログイン項目を開くよう依頼する（開いたかは確かめない）。
//
// エラーの分類は NSError の code で行う（ServiceManagement/SMErrors.h の kSMError* の値:
// 3 = InvalidSignature / 4 = AuthorizationFailure / 11 = LaunchDeniedByUser。SDK ヘッダと照合済み）。
// ドメイン名は一次情報で未照合のため照合しない。分類が外れても「その他（5）」へ倒れ、成功は作らない。
// 登録の成否は可能な限り status の読み戻しで決める（エラー文言に頼らない）。
// 参照: https://developer.apple.com/documentation/servicemanagement/smappservice
//
// .app 判定: 素の実行体にも Info.plist が埋め込まれ bundleIdentifier が取れてしまうため、
// bundleURL の拡張子が "app" かで判定する。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは受け取っても保持しない。ログは出さない
// （実行体のパス・NSError の文言を外へ出さない）。

#import <Foundation/Foundation.h>
#import <ServiceManagement/ServiceManagement.h>
#include <stdint.h>

// 前提を満たすなら 0、満たさないなら 1（macOS 13 未満）か 2（.app 外）。
static int32_t fandhe_launch_at_login_precheck(void) {
    if (@available(macOS 13.0, *)) {
        NSString *ext = [[[NSBundle mainBundle] bundleURL] pathExtension];
        if (![ext isEqualToString:@"app"]) {
            return 2;
        }
        return 0;
    }
    return 1;
}

static int32_t fandhe_launch_at_login_error_code(NSError *error) {
    switch (error.code) {
        case 4:  // kSMErrorAuthorizationFailure
        case 11: // kSMErrorLaunchDeniedByUser
            return 3;
        case 3:  // kSMErrorInvalidSignature
            return 4;
        default:
            return 5;
    }
}

int32_t fandhe_macos_launch_at_login_status(int32_t *out_status) {
    @autoreleasepool {
        int32_t pre = fandhe_launch_at_login_precheck();
        if (pre != 0) {
            return pre;
        }
        if (@available(macOS 13.0, *)) {
            *out_status = (int32_t)SMAppService.mainAppService.status;
            return 0;
        }
        return 1;
    }
}

int32_t fandhe_macos_launch_at_login_enable(int32_t *out_status) {
    @autoreleasepool {
        int32_t pre = fandhe_launch_at_login_precheck();
        if (pre != 0) {
            return pre;
        }
        if (@available(macOS 13.0, *)) {
            SMAppService *service = SMAppService.mainAppService;
            NSError *error = nil;
            if (![service registerAndReturnError:&error]) {
                SMAppServiceStatus now = service.status;
                // 既に登録済み・承認待ちで返るエラーは成功扱い（読み戻した値が真実）。
                if (now != SMAppServiceStatusEnabled && now != SMAppServiceStatusRequiresApproval) {
                    return fandhe_launch_at_login_error_code(error);
                }
            }
            *out_status = (int32_t)service.status;
            return 0;
        }
        return 1;
    }
}

int32_t fandhe_macos_launch_at_login_disable(int32_t *out_status) {
    @autoreleasepool {
        int32_t pre = fandhe_launch_at_login_precheck();
        if (pre != 0) {
            return pre;
        }
        if (@available(macOS 13.0, *)) {
            SMAppService *service = SMAppService.mainAppService;
            if (service.status == SMAppServiceStatusNotRegistered) {
                *out_status = (int32_t)SMAppServiceStatusNotRegistered;
                return 0;
            }
            NSError *error = nil;
            if (![service unregisterAndReturnError:&error]) {
                if (service.status != SMAppServiceStatusNotRegistered) {
                    return fandhe_launch_at_login_error_code(error);
                }
            }
            *out_status = (int32_t)service.status;
            return 0;
        }
        return 1;
    }
}

int32_t fandhe_macos_launch_at_login_open_settings(void) {
    @autoreleasepool {
        int32_t pre = fandhe_launch_at_login_precheck();
        if (pre != 0) {
            return pre;
        }
        if (@available(macOS 13.0, *)) {
            [SMAppService openSystemSettingsLoginItems];
            return 0;
        }
        return 1;
    }
}
