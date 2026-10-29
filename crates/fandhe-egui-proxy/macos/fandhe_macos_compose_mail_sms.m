// Phase 11 の拡充ループ（親 #1142）型サンプル「メール・SMS の作成」: macOS 実装（Issue #1384）。
// `NSWorkspace openURL:` で `mailto:` / `sms:` の URL を開き、既定のメール / メッセージアプリの
// 作成画面を出す。添付付きのメールは `NSSharingService`（composeEmail）で To・件名・本文・添付を渡す
// （Issue #1042）。権限は不要。
//
// `crates/fandhe-egui-proxy/src/device/macos_compose_mail_sms.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_compose_open_url(const char *url) -> int32_t
//     UI スレッド（main thread）から同期で呼ぶ。url は NUL 終端の UTF-8 で、Rust 側で検査・
//     percent-encode 済みの `mailto:` / `sms:` URL だけが渡される。ポインタは呼び出し中だけ読み、
//     保持しない。
//     0 = OS が URL を受理した（開いたか・送ったかは不明）/ 1 = 扱うアプリが無い（openURL: が NO）
//     2 = NSURL を作れない・main thread ではない
//
//   fandhe_macos_compose_attachment(const char *to_nl, const char *subject, const char *body,
//                                   const char *path) -> int32_t
//     main thread から同期で呼ぶ。to_nl は宛先を `\n` で連結した UTF-8（検証で宛先に `\n` は現れない）。
//     path は Rust が作った一時ファイル。ポインタは呼び出し中だけ読み、保持しない。
//     `recipients` / `subject` / `performWithItems:` の前提（Mail 以外が既定のとき
//     `canPerformWithItems:` が NO になる等）は Mac 目視（#1039）で確かめる。
//     完了の delegate は待たない（取り消しの通知が無く、推測のタイマーで「キャンセル」「送った」を
//     作らない）。
//     0 = 作成画面を出す依頼を OS が受けた / 1 = 共有できない（canPerformWithItems: が NO）
//     2 = 引数不正・main thread ではない
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ログは出さない（宛先・件名・本文を含むため）。

#import <AppKit/AppKit.h>
#include <stdint.h>

// 作成画面が閉じるまで service が解放されないよう、直近の 1 件を保持する（公式に保持要否の記載が無いため安全側）。
static NSSharingService *s_compose_service = nil;

int32_t fandhe_macos_compose_open_url(const char *url) {
    @autoreleasepool {
        if (![NSThread isMainThread] || url == NULL) {
            return 2;
        }
        NSString *s = [NSString stringWithUTF8String:url];
        NSURL *u = s == nil ? nil : [NSURL URLWithString:s];
        if (u == nil) {
            return 2;
        }
        return [[NSWorkspace sharedWorkspace] openURL:u] ? 0 : 1;
    }
}

int32_t fandhe_macos_compose_attachment(const char *to_nl, const char *subject, const char *body,
                                        const char *path) {
    @autoreleasepool {
        if (![NSThread isMainThread] || to_nl == NULL || subject == NULL || body == NULL ||
            path == NULL) {
            return 2;
        }
        NSString *t = [NSString stringWithUTF8String:to_nl];
        NSString *sub = [NSString stringWithUTF8String:subject];
        NSString *b = [NSString stringWithUTF8String:body];
        NSString *p = [NSString stringWithUTF8String:path];
        if (t == nil || sub == nil || b == nil || p == nil) {
            return 2;
        }
        NSSharingService *svc =
            [NSSharingService sharingServiceNamed:NSSharingServiceNameComposeEmail];
        if (svc == nil) {
            return 1;
        }
        NSArray *items = @[ b, [NSURL fileURLWithPath:p isDirectory:NO] ];
        if (![svc canPerformWithItems:items]) {
            return 1;
        }
        svc.recipients = t.length == 0 ? @[] : [t componentsSeparatedByString:@"\n"];
        svc.subject = sub;
        s_compose_service = svc;
        [svc performWithItems:items];
        return 0;
    }
}
