// Phase 11 の拡充ループ（親 #896）型サンプル「共有」: macOS 実装（Issue #897）+
// iOS 実装（Issue #899、`UIActivityViewController`）。iOS / macOS 共用ファイル
// （`fandhe_apple_location.m` と同じ判断）。
//
// `crates/fandhe-egui-proxy/src/device/apple_share.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS 全体・macOS ターゲット（feature `device` 有効時）で `cc`
// コンパイルする。macOS は AppKit を、iOS は UIKit をリンクする。
//
// 契約（pull 型。`fandhe_apple_location.m` と同型、coding-rust.md の FFI 例外規定）:
//   fandhe_apple_share_present(uint64_t request_id, const char *text, const char *url_or_null)
//     -> int32_t 戻り値: 0 = 提示依頼を受理（結果は非同期に fandhe_rust_share_outcome
//        で届く）。1 = 提示依頼自体の失敗（呼び出し元が同期的に Internal エラーへ
//        畳む）。text/url は NUL 終端 UTF-8。関数内で即座に NSString へコピーして
//        から return するため、呼び出し元は return 後に元の文字列バッファを解放して
//        よい（所有権は移らない）。
//   fandhe_rust_share_outcome(uint64_t request_id, int32_t code) -> Rust 側
//     （`apple_share.rs`）が公開する `extern "C"` 関数。結果コードの意味は
//     `device/share.rs::outcome_from_code` の表（iOS/macOS/Android 共通、Issue #899
//     §2.2）を参照。macOS: 0=共有した（service 非 nil）/1=キャンセル（service nil）/
//     2=共有先なし/3=内部エラー（ウィンドウ不在等）。iOS: 0=共有した（completed==YES）/
//     1=キャンセル（completed==NO && error==nil）/3=内部エラー（VC 不在・error!=nil・
//     URL 解釈失敗）。iOS は 2（NoTarget）を使わない（`UIActivityViewController` は
//     コピー等が常に候補にあるため「共有先なし」判定を行わない）。
//
// スレッド: `dispatch_async(main)` で共有 UI の提示自体を行う（`fandhe_ios_file_picker.m`
// と同じ方針）。egui の update 内で AppKit/UIKit のネストした run loop を回さないため。
// delegate コールバックはメインスレッドで届き、そこから直接 Rust 側 `extern "C"` 関数を
// 呼ぶ（iOS は `apple_share.rs::fandhe_rust_share_outcome` が
// `crate::ios::request_repaint()` で明示的に次フレームを起こす）。
//
// 共有テキスト・URL をログへ出力しない（security.md A09）。
//
// ponytail: サービス側の共有失敗（macOS `NSSharingServiceDelegate` の
// `didFailToShareItems:`）は拾わない（「選んだ」時点で Shared 扱いにする）。
// 要件化したら `delegateForSharingService:` を実装して失敗コードを追加する。

#import <TargetConditionals.h>
#if TARGET_OS_OSX
#import <AppKit/AppKit.h>
#else
#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"
#endif

// Rust 側（`apple_share.rs`）が公開する `extern "C"` 関数。両 OS 共通。
extern void fandhe_rust_share_outcome(uint64_t request_id, int32_t code);

// `NSURL URLWithString:` は未エンコードの非 ASCII 文字を含む文字列に対し nil を返す
// （国際化されたパスは現実的な入力）。まず素通しで試し、失敗した場合のみ authority
// （userinfo/host/port）と残り（パス/クエリ/フラグメント）を分けて組み立て直す。
// host は `NSURLComponents` の `host` セッター経由で IDNA（punycode）変換させ、
// 残りだけ percent-encode する（Bugbot 指摘: 文字列全体を percent-encode すると
// IDN ホストが punycode（`xn--...`）ではなく `%E4%BE%8B` のような無効な形になり
// 解決できないため）。
//
// Bugbot 指摘 (Double-slash paths rewrite shared URL): 残り文字列を
// `URLWithString:relativeToURL:` で base に対し相対解決すると、残りが "//" で
// 始まる場合 RFC 3986 の network-path reference として解釈され host が丸ごと
// 置き換わる。相対解決を使わず、authority を含む絶対文字列へ直接連結してから
// 1 回でパースすることで回避する。
//
// Bugbot 指摘 (IDN share URLs drop port): host 抽出が "/?#" までを丸ごと host に
// 入れていたため、port・userinfo を含む authority（例: "host:8080"）を渡すと
// host セッターの IDNA 変換が失敗しうる。userinfo（"@" 区切り）と port（末尾の
// ":<数字>"）を先に分離し、host には素の host 文字列だけを渡す。
// userinfo の user/password 各部分が `percentEncodedUser` / `percentEncodedPassword`
// セッターへそのまま渡せる形（英数字・sub-delims・"%XX" の pct-encoded 三つ組のみ）
// かを検証する。この 2 セッターは事前に正しく percent-encode 済みの文字列を要求し、
// 単独の "%" や不正な hex を渡すと NSInvalidArgumentException で例外送出するため
// （codex-review P1 / Bugbot 指摘: userinfo に不正な "%" を含む共有 URL でクラッシュ）、
// 例外を踏む前にここで弾いて回復可能なエラー（nil → 呼び出し元がエラー扱い）に倒す。
static BOOL FandheIsValidPercentEncodedUserinfoComponent(NSString *component) {
    static NSCharacterSet *allowed;
    static NSCharacterSet *hexDigits;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        allowed = [NSCharacterSet characterSetWithCharactersInString:
            @"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~!$&'()*+,;="];
        hexDigits = [NSCharacterSet characterSetWithCharactersInString:@"0123456789ABCDEFabcdef"];
    });
    NSUInteger length = component.length;
    for (NSUInteger i = 0; i < length; i++) {
        unichar c = [component characterAtIndex:i];
        if (c == '%') {
            if (i + 2 >= length || ![hexDigits characterIsMember:[component characterAtIndex:i + 1]] ||
                ![hexDigits characterIsMember:[component characterAtIndex:i + 2]]) {
                return NO;
            }
            i += 2;
            continue;
        }
        if (![allowed characterIsMember:c]) {
            return NO;
        }
    }
    return YES;
}

static NSURL *FandheURLFromString(NSString *urlString) {
    NSURL *url = [NSURL URLWithString:urlString];
    if (url != nil) {
        return url;
    }

    NSRange schemeSep = [urlString rangeOfString:@"://"];
    if (schemeSep.location == NSNotFound) {
        return nil;
    }
    NSString *scheme = [urlString substringToIndex:schemeSep.location];
    NSString *afterScheme = [urlString substringFromIndex:NSMaxRange(schemeSep)];

    NSCharacterSet *hostEnd = [NSCharacterSet characterSetWithCharactersInString:@"/?#"];
    NSRange restRange = [afterScheme rangeOfCharacterFromSet:hostEnd];
    NSString *authority = (restRange.location == NSNotFound) ? afterScheme : [afterScheme substringToIndex:restRange.location];
    NSString *rest = (restRange.location == NSNotFound) ? @"" : [afterScheme substringFromIndex:restRange.location];
    if (authority.length == 0) {
        return nil;
    }

    // userinfo（"user[:pass]@"）を host:port から分離する。
    NSString *userinfo = nil;
    NSString *hostport = authority;
    NSRange atRange = [authority rangeOfString:@"@"];
    if (atRange.location != NSNotFound) {
        userinfo = [authority substringToIndex:atRange.location];
        hostport = [authority substringFromIndex:NSMaxRange(atRange)];
    }

    // port（末尾の ":<数字だけ>"）を host から分離する。IPv6 の "[...]" 表記は
    // 対象外（ponytail: IPv6 の共有 URL が要件化したら "[" 始まりを別扱いする）。
    NSString *host = hostport;
    NSString *portString = nil;
    NSRange colonRange = [hostport rangeOfString:@":" options:NSBackwardsSearch];
    if (colonRange.location != NSNotFound && ![hostport hasPrefix:@"["]) {
        NSString *candidatePort = [hostport substringFromIndex:NSMaxRange(colonRange)];
        NSCharacterSet *nonDigit = [NSCharacterSet characterSetWithCharactersInString:@"0123456789"].invertedSet;
        // 桁数を 5 桁以下に制限したうえで数値化し、0-65535 の範囲外なら
        // ポート指定として扱わない（NSURLComponents.port は範囲外で例外送出するため、
        // ここで弾かないと共有 URL 由来でメインスレッドがクラッシュしうる）。
        if (candidatePort.length > 0 && candidatePort.length <= 5 &&
            [candidatePort rangeOfCharacterFromSet:nonDigit].location == NSNotFound &&
            [candidatePort integerValue] <= 65535) {
            host = [hostport substringToIndex:colonRange.location];
            portString = candidatePort;
        }
    }
    if (host.length == 0) {
        return nil;
    }

    NSURLComponents *comps = [[NSURLComponents alloc] init];
    comps.scheme = scheme;
    if (userinfo != nil) {
        NSRange userColon = [userinfo rangeOfString:@":"];
        NSString *userPart = (userColon.location != NSNotFound) ? [userinfo substringToIndex:userColon.location] : userinfo;
        NSString *passwordPart = (userColon.location != NSNotFound) ? [userinfo substringFromIndex:NSMaxRange(userColon)] : nil;
        if (!FandheIsValidPercentEncodedUserinfoComponent(userPart) ||
            (passwordPart != nil && !FandheIsValidPercentEncodedUserinfoComponent(passwordPart))) {
            // 不正な percent-encoding は percentEncodedUser/Password セッターが
            // 例外送出するため、ここで回復可能なエラーとして扱う（nil を返す）。
            return nil;
        }
        comps.percentEncodedUser = userPart;
        if (passwordPart != nil) {
            comps.percentEncodedPassword = passwordPart;
        }
    }
    comps.host = host;
    if (portString != nil) {
        comps.port = @([portString integerValue]);
    }
    NSString *authorityString = comps.string;
    if (authorityString == nil) {
        return nil;
    }
    if (rest.length == 0) {
        return comps.URL;
    }

    NSCharacterSet *allowed = [NSCharacterSet characterSetWithCharactersInString:
        @"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~:/?#[]@!$&'()*+,;=%"];
    NSString *encodedRest = [rest stringByAddingPercentEncodingWithAllowedCharacters:allowed];
    if (encodedRest == nil) {
        return nil;
    }
    // authorityString（"scheme://host[:port]"）へ直接連結して 1 回でパースする
    // （relativeToURL は使わない。上記コメント参照）。
    return [NSURL URLWithString:[authorityString stringByAppendingString:encodedRest]];
}

#if TARGET_OS_OSX

// delegate はコールバックが届くまでプロセス内で生存させる必要があるため static で
// 強参照する（`NSSharingServicePicker` 自体は delegate を弱参照するため）。
static id<NSSharingServicePickerDelegate> g_share_delegate = nil;

@interface FandheShareDelegate : NSObject <NSSharingServicePickerDelegate>
@property(nonatomic, assign) uint64_t requestId;
@end

@implementation FandheShareDelegate

- (void)sharingServicePicker:(NSSharingServicePicker *)sharingServicePicker
       didChooseSharingService:(NSSharingService *)service {
    int32_t code = (service != nil) ? 0 : 1;
    fandhe_rust_share_outcome(self.requestId, code);
    // 通知後は自分を強参照している static を解放する（以降 self に触れない）。
    g_share_delegate = nil;
}

@end


static void FandhePresentShareOnMainThread(uint64_t request_id, NSString *text, NSString *urlString) {
    NSArray<NSSharingService *> *availableServices;
    NSMutableArray<id> *items = [NSMutableArray arrayWithObject:text];
    NSURL *url = (urlString != nil) ? FandheURLFromString(urlString) : nil;
    if (urlString != nil && url == nil) {
        // URL 指定があったのに解釈できない場合、テキストのみで黙って共有せず
        // エラーとして呼び出し元へ返す（Bugbot 指摘: 非 ASCII URL の黙落防止）。
        fandhe_rust_share_outcome(request_id, 3);
        return;
    }
    if (url != nil) {
        [items addObject:url];
    }

    // macOS 13+ で `sharingServicesForItems:` は非推奨（`standardShareMenuItem` 推奨）
    // だが、「共有先が無い」を picker 提示前に判定する唯一の同期 API のため使い続ける
    // （`.m` ファイル冒頭コメント参照）。警告のみ抑止する。
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    availableServices = [NSSharingService sharingServicesForItems:items];
#pragma clang diagnostic pop
    if (availableServices.count == 0) {
        fandhe_rust_share_outcome(request_id, 2);
        return;
    }

    NSWindow *window = NSApp.keyWindow ?: NSApp.mainWindow ?: NSApp.windows.firstObject;
    NSView *view = window.contentView;
    if (view == nil) {
        fandhe_rust_share_outcome(request_id, 3);
        return;
    }

    NSSharingServicePicker *picker = [[NSSharingServicePicker alloc] initWithItems:items];
    FandheShareDelegate *delegate = [[FandheShareDelegate alloc] init];
    delegate.requestId = request_id;
    g_share_delegate = delegate;
    picker.delegate = delegate;

    [picker showRelativeToRect:view.bounds ofView:view preferredEdge:NSMinYEdge];
}

int32_t fandhe_apple_share_present(uint64_t request_id, const char *text, const char *url_or_null) {
    if (text == NULL) {
        return 1;
    }
    NSString *nsText = [NSString stringWithUTF8String:text];
    NSString *nsUrl = (url_or_null != NULL) ? [NSString stringWithUTF8String:url_or_null] : nil;
    if (nsText == nil) {
        return 1;
    }

    dispatch_async(dispatch_get_main_queue(), ^{
        FandhePresentShareOnMainThread(request_id, nsText, nsUrl);
    });
    return 0;
}

#else

// iOS 実装（Issue #899）。`UIActivityViewController` を最前面 VC（`fandhe_ios_common.h`
// の `FandheTopmostViewController`）から提示する。iPad では popover の
// `sourceView`/`sourceRect` を必ず設定する（未設定はクラッシュ、実装計画 R2）。

static void FandhePresentShareOnMainThread(uint64_t request_id, NSString *text, NSString *urlString) {
    NSMutableArray<id> *items = [NSMutableArray arrayWithObject:text];
    NSURL *url = (urlString != nil) ? FandheURLFromString(urlString) : nil;
    if (urlString != nil && url == nil) {
        // URL 指定があったのに解釈できない場合、テキストのみで黙って共有せず
        // エラーとして呼び出し元へ返す（macOS 経路と同じ方針、Bugbot 指摘参照）。
        fandhe_rust_share_outcome(request_id, 3);
        return;
    }
    if (url != nil) {
        [items addObject:url];
    }

    UIViewController *top = FandheTopmostViewController();
    if (top == nil || top.view.window == nil) {
        // `presentViewController` がデリゲートを一切呼ばずに失敗する既知の失敗系
        // （`fandhe_ios_camera.m`/`fandhe_ios_file_picker.m` と同じ判定）。
        fandhe_rust_share_outcome(request_id, 3);
        return;
    }

    UIActivityViewController *avc =
        [[UIActivityViewController alloc] initWithActivityItems:items applicationActivities:nil];

    // iPad ではフルスクリーンではなく popover 提示になり、`sourceView`/`sourceRect`
    // 未設定だと `presentViewController` がクラッシュする（実装計画 R2）。iPhone では
    // popoverPresentationController が nil レシーバとして無視されるため無害
    // （`permittedArrowDirections = 0` は矢印を出さない = 中央からのカード提示に揃える）。
    avc.popoverPresentationController.sourceView = top.view;
    avc.popoverPresentationController.sourceRect =
        CGRectMake(CGRectGetMidX(top.view.bounds), CGRectGetMidY(top.view.bounds), 1, 1);
    avc.popoverPresentationController.permittedArrowDirections = 0;

    __block BOOL handled = NO;
    avc.completionWithItemsHandler =
        ^(UIActivityType _Nullable __unused activityType, BOOL completed,
          NSArray *_Nullable __unused returnedItems, NSError *_Nullable activityError) {
          // ponytail: iOS は `UIActivityViewController` 提示前に「共有先が無い」を
          // 判定しない（常にコピー等の標準アクティビティが候補にあるため、macOS の
          // `sharingServicesForItems:` に相当する事前判定 API が無い）。NoTarget
          // （code 2）はこの経路では発生しない。
          if (handled) {
              // completion は一度しか呼ばれない契約（UIKit ドキュメント）だが、
              // 二重呼び出しへの安全側の防御として畳む（`bridge::deliver` の
              // first-wins と二重防御、file_picker 系と同じ判断）。
              return;
          }
          handled = YES;
          int32_t code = (activityError != nil) ? 3 : (completed ? 0 : 1);
          fandhe_rust_share_outcome(request_id, code);
        };

    [top presentViewController:avc animated:YES completion:nil];
}

static void FandheDispatchPresent(uint64_t request_id, NSString *text, NSString *urlString) {
    dispatch_async(dispatch_get_main_queue(), ^{
      FandhePresentShareOnMainThread(request_id, text, urlString);
    });
}

int32_t fandhe_apple_share_present(uint64_t request_id, const char *text, const char *url_or_null) {
    if (text == NULL) {
        return 1;
    }
    NSString *nsText = [NSString stringWithUTF8String:text];
    NSString *nsUrl = (url_or_null != NULL) ? [NSString stringWithUTF8String:url_or_null] : nil;
    if (nsText == nil) {
        return 1;
    }

    FandheDispatchPresent(request_id, nsText, nsUrl);
    return 0;
}

#endif
