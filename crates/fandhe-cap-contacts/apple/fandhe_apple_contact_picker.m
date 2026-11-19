// Phase 11 の拡充ループ（親 #1117）型サンプル「連絡先選択」: macOS（Issue #1220）+ iOS（Issue #1221）実装。
//
// `crates/fandhe-cap-contacts/src/apple.rs` から C ABI 経由で呼ばれる。
// `build.rs` が macOS / iOS ターゲットで `cc` コンパイルし、
// Contacts / ContactsUI をリンクする。
//
// 契約（pull 型。`fandhe_apple_biometric.m` と同型、coding-rust.md の FFI 例外規定）:
//   fandhe_apple_contact_picker_present(uint64_t request_id) -> int32_t
//     戻り値: 0 = 提示依頼を受理（結果は非同期に fandhe_rust_contact_picker_outcome で届く）。
//   fandhe_rust_contact_picker_outcome(uint64_t request_id, int32_t code, const char *name)
//     -> Rust 側（本 crate の `apple.rs`）が公開する `extern "C"` 関数。
//        code 表: 0 = 選択（name は NUL 終端 UTF-8。空文字列もありうる）、1 = キャンセル、
//        2 = 権限拒否、3 = 提示失敗、4 = 提示先のシーンが切断された（iOS のみ。完了通知を失った要求の確定）。name は code 0 のときだけ非 NULL で、呼び出しの間だけ
//        有効（Rust 側が呼び出し中にコピーする）。コード → 結果の写像は Rust 側
//        本 crate の `driver.rs::outcome_from_code` に一本化する（`.m` 側で二重に写像しない）。
//
// 権限: `CNContactPicker`（popover）が TCC を要するかは公式文書に明記がないため安全側に倒し、
// 提示前に `authorizationStatusForEntityType:` を読む（読むだけでは許可ダイアログは出ない）。
// Denied / Restricted は code 2。NotDetermined は `requestAccessForEntityType:` で許可を求め、
// 拒否（granted=NO）は error の有無を見ず、認可状態を読み直す。Denied / Restricted なら code 2
// （初回の拒否は error 付きで返るため）、それ以外で error ありは code 3。それ以外（Authorized と
// Limited）は提示する。ピッカーだけなら不要と Mac 目視（#1213）で分かった場合は、この判定を外す。
//
// スレッド: 提示・static へのアクセスはすべて main queue に限る。許可の completion は任意
// スレッドで届くため main queue へ戻してから続きを行う。
//
// iOS（#1221）: `CNContactPickerViewController` を最前面の VC から提示する（プロセス外で動く
// ため権限は不要。認可状態の確認・要求はせず code 2 も出さない。`NSContactsUsageDescription`
// も足さない）。`contactPicker:didSelectContacts:`（複数形）は実装しない（実装すると複数選択
// になる）。キャンセルボタンは `contactPickerDidCancel:`、スワイプ dismiss は
// `presentationControllerDidDismiss:` で code 1。提示先 VC が無ければ code 3。
//
// シーンの切断（iOS）: 完了通知を失う経路のうちシーンの切断は、`UISceneDidDisconnectNotification` の観測で失敗（code 4）として
// 1 度だけ確定する（file-picker・photo-picker と同じ形。画面は `Idle` 以外で次の要求を受けないため、残すと `Picking` のまま止まる）。
// 提示先のシーンが特定できない・シーンが接続されたまま通知が来ない場合は観測する通知が無く、残る（天井）。
// 置き換えで古いピッカーの dismiss の完了を待つ間に切断され、completion が来ない場合も、新しい要求は提示前で記録が無く残る
// （`animated:NO` で窓はごく狭い。画面と公開入口を併用したときだけ起きる）。
//
// 再要求は置き換え: 保留中のピッカーがあれば古いデリゲートを「通知済み」にしてから閉じる
// （古い要求は Rust 側で要求 id の不一致により既に無効）。許可待ちの間に新しい要求が来た場合も、
// 再開時に `g_latest_request_id` と照合して古い要求は何も通知せず捨てる。
// 完了通知は 1 要求につき 1 回（デリゲートの `delivered` で守る）。
//
// 情報非保持: 通知するのは表示名だけ。電話・メール・識別子は読まない。`displayedKeys` は
// 設定しない（非空だと連絡先でなくプロパティの選択表示になり didSelectContact: が呼ばれない）。`NSLog` を含むログ出力は一切行わない（security.md A09）。
// ObjC の例外を Rust へ通さないため `@try/@catch` に頼らず、未取得キーへの接触は
// `areKeysAvailable:` で事前に避ける。

#import <TargetConditionals.h>

#if TARGET_OS_OSX
#import <AppKit/AppKit.h>
#elif TARGET_OS_IOS
#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"
#endif

#if TARGET_OS_OSX || TARGET_OS_IOS

#import <Contacts/Contacts.h>
#import <ContactsUI/ContactsUI.h>

extern void fandhe_rust_contact_picker_outcome(uint64_t request_id, int32_t code, const char *name);

#if TARGET_OS_IOS
@interface FandheContactPickerDelegate : NSObject <CNContactPickerDelegate, UIAdaptivePresentationControllerDelegate>
#else
@interface FandheContactPickerDelegate : NSObject <CNContactPickerDelegate>
#endif
@property(nonatomic) uint64_t requestId;
@property(nonatomic) BOOL delivered;
@end

// 保留中のピッカーとデリゲート。reply が届くまで強参照で生存させる。main queue 限定。
#if TARGET_OS_IOS
static CNContactPickerViewController *g_picker = nil;
#else
static CNContactPicker *g_picker = nil;
#endif
static FandheContactPickerDelegate *g_delegate = nil;
static uint64_t g_latest_request_id = 0;
#if TARGET_OS_IOS
// 提示したシーン（weak）。シーンの切断（`UISceneDidDisconnectNotification`）が、その要求の完了通知を失った観測になる。
// 提示の直前に記録し、`g_delegate` を nil にする箇所ですべて nil にする。main queue 限定。
static __weak UIScene *g_activeScene = nil;
// 切断通知の購読トークン。強参照で持ち続けないと observer が解放されて購読が無効になる。main queue 限定。
static id _Nullable g_sceneDisconnectToken = nil;
#endif

// 完了通知を 1 回だけ行う。
static void FandheContactNotify(FandheContactPickerDelegate *delegate, int32_t code, NSString *name) {
    if (delegate.delivered) {
        return;
    }
    delegate.delivered = YES;
    fandhe_rust_contact_picker_outcome(delegate.requestId, code, name != nil ? name.UTF8String : NULL);
}

// 自分が現在のピッカーのときだけ static を解放する（置き換え後の古い close で新しい方を消さない）。
static void FandheContactReleaseIfCurrent(FandheContactPickerDelegate *delegate) {
    if (g_delegate == delegate) {
        g_delegate = nil;
        g_picker = nil;
#if TARGET_OS_IOS
        g_activeScene = nil;
#endif
    }
}

// 表示名を作る。氏名（ロケール順）→ 会社名 → 空文字列（Rust 側が空名として失敗扱いにする）の順。
// CNContactFormatter は追加キーの取得を要するため使わず、取得済みの姓名から NSPersonNameComponentsFormatter で組み立てる。
static NSString *FandheContactDisplayName(CNContact *contact) {
    NSString *given = [contact isKeyAvailable:CNContactGivenNameKey] ? contact.givenName : nil;
    NSString *family = [contact isKeyAvailable:CNContactFamilyNameKey] ? contact.familyName : nil;
    if (given.length > 0 || family.length > 0) {
        NSPersonNameComponents *components = [[NSPersonNameComponents alloc] init];
        components.givenName = given;
        components.familyName = family;
        NSString *full = [NSPersonNameComponentsFormatter localizedStringFromPersonNameComponents:components
                                                                                            style:NSPersonNameComponentsFormatterStyleDefault
                                                                                          options:0];
        if ([full stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet].length > 0) {
            return full;
        }
    }
    if ([contact isKeyAvailable:CNContactOrganizationNameKey]) {
        return contact.organizationName ?: @"";
    }
    return @"";
}

#if TARGET_OS_IOS
@implementation FandheContactPickerDelegate

- (void)contactPicker:(CNContactPickerViewController *)picker didSelectContact:(CNContact *)contact {
    FandheContactNotify(self, 0, FandheContactDisplayName(contact));
    FandheContactReleaseIfCurrent(self);
}

- (void)contactPickerDidCancel:(CNContactPickerViewController *)picker {
    FandheContactNotify(self, 1, nil);
    FandheContactReleaseIfCurrent(self);
}

// スワイプ dismiss。選択・キャンセル後に届いても `delivered` で 1 回に絞られる。
- (void)presentationControllerDidDismiss:(UIPresentationController *)presentationController {
    FandheContactNotify(self, 1, nil);
    FandheContactReleaseIfCurrent(self);
}

@end

// シーンの切断で、完了通知を失った要求を失敗（code 4）で 1 度だけ確定する（main queue）。
// 切断されるとピッカーは delegate を呼ばずに消える、という挙動は公式文書で確認されておらず実機で未確認。
// OS の観測で確定するので推測のタイマーは持たない。提示したシーンが記録でき、それが切断されたときだけ確定する
// （記録が nil なら、iPad のマルチウィンドウで無関係な別シーンが閉じただけで正しい選択を落とさないよう確定しない）。
// 選択・キャンセルで確定済みの要求は先に `g_delegate` が nil になっているので何もしない。
static void FandheContactFailOnSceneDisconnect(UIScene *disconnected) {
    FandheContactPickerDelegate *stale = g_delegate;
    if (stale == nil) {
        return;
    }
    UIScene *active = g_activeScene;
    if (active == nil || active != disconnected) {
        return;
    }
    g_picker.presentationController.delegate = nil;
    g_picker.delegate = nil;
    g_delegate = nil;
    g_picker = nil;
    g_activeScene = nil;
    FandheContactNotify(stale, 4, nil);
}

// 切断通知の購読を 1 度だけ登録する（main queue）。
static void FandheContactObserveSceneDisconnect(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      g_sceneDisconnectToken = [NSNotificationCenter.defaultCenter
          addObserverForName:UISceneDidDisconnectNotification
                      object:nil
                       queue:NSOperationQueue.mainQueue
                  usingBlock:^(NSNotification *note) {
                    @autoreleasepool {
                        if ([note.object isKindOfClass:UIScene.class]) {
                            FandheContactFailOnSceneDisconnect((UIScene *)note.object);
                        }
                    }
                  }];
    });
}

static void FandheContactShow(uint64_t request_id) {
    UIViewController *top = FandheTopmostViewController();
    if (top == nil || top.view.window == nil) {
        fandhe_rust_contact_picker_outcome(request_id, 3, NULL);
        return;
    }

    FandheContactPickerDelegate *delegate = [[FandheContactPickerDelegate alloc] init];
    delegate.requestId = request_id;
    CNContactPickerViewController *picker = [[CNContactPickerViewController alloc] init];
    picker.delegate = delegate; // weak のため g_delegate の強参照で生かす。
    g_delegate = delegate;
    g_picker = picker;
    g_activeScene = top.view.window.windowScene;
    [top presentViewController:picker animated:YES completion:^{
      // スワイプ dismiss の通知先。提示後に設定する（提示前は presentationController が無い場合がある）。
      picker.presentationController.delegate = delegate;
    }];
}

static void FandheContactPresentOnMain(uint64_t request_id) {
    g_latest_request_id = request_id;
    FandheContactObserveSceneDisconnect();

    // 認可状態の確認・要求はしない（ピッカーはプロセス外で動き権限不要。code 2 は出さない）。
    if (g_delegate == nil) {
        FandheContactShow(request_id);
        return;
    }
    // 置き換え: 保留中のピッカーを閉じてから新しい要求を提示する（古い要求は通知しない）。
    g_delegate.delivered = YES;
    CNContactPickerViewController *old = g_picker;
    g_delegate = nil;
    g_picker = nil;
    g_activeScene = nil;
    UIViewController *presenting = old.presentingViewController;
    if (presenting == nil) {
        // 古いピッカーが提示に失敗した等で閉じる対象が無い。nil へ送った dismiss は completion が
        // 呼ばれず新しい要求がハングするため、待たずに直接提示する。
        FandheContactShow(request_id);
        return;
    }
    [presenting dismissViewControllerAnimated:NO
                                   completion:^{
                                     if (request_id == g_latest_request_id) {
                                         FandheContactShow(request_id);
                                     }
                                   }];
}
#else // macOS

@implementation FandheContactPickerDelegate

- (void)contactPicker:(CNContactPicker *)picker didSelectContact:(CNContact *)contact {
    FandheContactNotify(self, 0, FandheContactDisplayName(contact));
}

- (void)contactPickerDidClose:(CNContactPicker *)picker {
    // 選択後の close ではすでに通知済み。選ばずに閉じられたときだけキャンセル。
    FandheContactNotify(self, 1, nil);
    FandheContactReleaseIfCurrent(self);
}

@end

static void FandheContactShow(uint64_t request_id) {
    NSWindow *window = NSApp.keyWindow ?: NSApp.mainWindow ?: NSApp.windows.firstObject;
    NSView *view = window.contentView;
    if (view == nil) {
        fandhe_rust_contact_picker_outcome(request_id, 3, NULL);
        return;
    }

    FandheContactPickerDelegate *delegate = [[FandheContactPickerDelegate alloc] init];
    delegate.requestId = request_id;
    CNContactPicker *picker = [[CNContactPicker alloc] init];
    picker.delegate = delegate;
    g_delegate = delegate;
    g_picker = picker;
    [picker showRelativeToRect:view.bounds ofView:view preferredEdge:NSMinYEdge];
}

static void FandheContactPresentOnMain(uint64_t request_id) {
    g_latest_request_id = request_id;

    // 置き換え: 保留中のピッカーを閉じる（古い要求は通知しない）。
    if (g_delegate != nil) {
        g_delegate.delivered = YES;
        CNContactPicker *old = g_picker;
        g_delegate = nil;
        g_picker = nil;
        [old close];
    }

    switch ([CNContactStore authorizationStatusForEntityType:CNEntityTypeContacts]) {
    case CNAuthorizationStatusDenied:
    case CNAuthorizationStatusRestricted:
        fandhe_rust_contact_picker_outcome(request_id, 2, NULL);
        return;
    case CNAuthorizationStatusNotDetermined: {
        CNContactStore *store = [[CNContactStore alloc] init];
        [store requestAccessForEntityType:CNEntityTypeContacts
                        completionHandler:^(BOOL granted, NSError *_Nullable error) {
                          // error は内容を読まず有無だけを使う（個人情報を含みうるため）。
                          BOOL failed = error != nil;
                          dispatch_async(dispatch_get_main_queue(), ^{
                            if (request_id != g_latest_request_id) {
                                return; // 許可待ちの間に置き換えられた。
                            }
                            if (granted) {
                                FandheContactShow(request_id);
                            } else {
                                // 拒否は error 付きでも返る。error ではなく実際の認可状態で判定する。
                                CNAuthorizationStatus now =
                                    [CNContactStore authorizationStatusForEntityType:CNEntityTypeContacts];
                                BOOL denied = now == CNAuthorizationStatusDenied ||
                                              now == CNAuthorizationStatusRestricted;
                                // 拒否でない失敗（通信失敗等）は提示失敗（code 3）。error なしは拒否扱い。
                                fandhe_rust_contact_picker_outcome(request_id,
                                                                   (denied || !failed) ? 2 : 3, NULL);
                            }
                          });
                        }];
        return;
    }
    default:
        FandheContactShow(request_id);
        return;
    }
}

#endif

int32_t fandhe_apple_contact_picker_present(uint64_t request_id) {
    dispatch_async(dispatch_get_main_queue(), ^{
      @autoreleasepool {
          FandheContactPresentOnMain(request_id);
      }
    });
    return 0;
}

#endif // TARGET_OS_OSX || TARGET_OS_IOS
