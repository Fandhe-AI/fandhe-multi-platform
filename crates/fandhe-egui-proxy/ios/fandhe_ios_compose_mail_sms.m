// iOS 向けメール・SMS 作成画面（Issue #1385）。`src/device/ios_compose_mail_sms.rs` から
// 呼ばれ、MessageUI の作成画面を main queue で提示する。権限・Info.plist キーは不要。
//
// C ABI:
//   int32_t fandhe_ios_compose_mail_sms_present(uint64_t request_id, int32_t kind,
//       const char *to, const char *cc, const char *bcc, const char *subject,
//       const char *body, const uint8_t *att_data, size_t att_len, const char *att_mime,
//       const char *att_name)
//     to / cc / bcc は宛先を `\n` で連結した文字列（空文字列 = 宛先なし。SMS の cc / bcc は空）。
//     att_* はメールの添付 1 件（Issue #32）。att_len == 0 は添付なし（att_data は NULL 可）。
//     att_len > 0 で data / mime / name のいずれかが NULL、または SMS に添付があれば 1 を返す。
//     添付は一時ファイルを作らず `addAttachmentData:` へ NSData で渡す。
//     kind: 0 = メール、1 = SMS。戻り値: 0 = 受理（結果は後で非同期に届く）、
//     1 = 受理できない（NULL・不正な UTF-8・未知の kind）。文字列・バイト列は return 前に
//     NSString / NSData へコピーし、所有権は移らない。
//   extern void fandhe_rust_compose_mail_sms_outcome(uint64_t request_id, int32_t code)
//     code は `device/compose_mail_sms.rs::outcome_from_ios_code` の表と一致させる
//     （0 Queued / 1 Saved / 2 Cancelled / 3 失敗 / 4 NoHandler / 5 提示失敗 / 99 未知）。
//     各依頼につきちょうど 1 回だけ呼ぶ。
//
// 宛先・件名・本文・添付のバイト列と名前・NSError はログへ出さない。

#import <MessageUI/MessageUI.h>
#import <UIKit/UIKit.h>
#import "fandhe_ios_common.h"

extern void fandhe_rust_compose_mail_sms_outcome(uint64_t request_id, int32_t code);

@interface FandheComposeDelegate : NSObject <MFMailComposeViewControllerDelegate,
                                             MFMessageComposeViewControllerDelegate>
@property(nonatomic) uint64_t requestId;
@property(nonatomic) BOOL done;
@end

// delegate の強参照。弱参照のままだと提示直後に解放され完了が届かない。
static FandheComposeDelegate *g_retainedDelegate = nil;

// `\n` 連結の宛先を配列へ戻す。空文字列・空要素は捨てる。
static NSArray<NSString *> *FandheSplitRecipients(NSString *joined) {
    NSMutableArray<NSString *> *out = [NSMutableArray array];
    for (NSString *s in [joined componentsSeparatedByString:@"\n"]) {
        if (s.length > 0) {
            [out addObject:s];
        }
    }
    return out;
}

@implementation FandheComposeDelegate

// 状態を確定（done・強参照解放）してから 1 回だけ結果を届ける。
- (void)finish:(UIViewController *)controller code:(int32_t)code {
    if (self.done) {
        return;
    }
    self.done = YES;
    uint64_t rid = self.requestId;
    g_retainedDelegate = nil;
    [controller dismissViewControllerAnimated:YES completion:nil];
    fandhe_rust_compose_mail_sms_outcome(rid, code);
}

- (void)mailComposeController:(MFMailComposeViewController *)controller
          didFinishWithResult:(MFMailComposeResult)result
                        error:(NSError *)error {
    int32_t code;
    switch (result) {
    case MFMailComposeResultSent: code = 0; break;
    case MFMailComposeResultSaved: code = 1; break;
    case MFMailComposeResultCancelled: code = 2; break;
    case MFMailComposeResultFailed: code = 3; break;
    default: code = 99; break;
    }
    [self finish:controller code:code];
}

- (void)messageComposeViewController:(MFMessageComposeViewController *)controller
                 didFinishWithResult:(MessageComposeResult)result {
    int32_t code;
    switch (result) {
    case MessageComposeResultSent: code = 0; break;
    case MessageComposeResultCancelled: code = 2; break;
    case MessageComposeResultFailed: code = 3; break;
    default: code = 99; break;
    }
    [self finish:controller code:code];
}

@end

int32_t fandhe_ios_compose_mail_sms_present(uint64_t request_id, int32_t kind, const char *to,
                                            const char *cc, const char *bcc,
                                            const char *subject, const char *body,
                                            const uint8_t *att_data, size_t att_len,
                                            const char *att_mime, const char *att_name) {
    if (to == NULL || cc == NULL || bcc == NULL || subject == NULL || body == NULL || (kind != 0 && kind != 1)) {
        return 1;
    }
    if (att_len > 0 && (att_data == NULL || att_mime == NULL || att_name == NULL || kind != 0)) {
        return 1;
    }
    NSString *toS = [NSString stringWithUTF8String:to];
    NSString *ccS = [NSString stringWithUTF8String:cc];
    NSString *bccS = [NSString stringWithUTF8String:bcc];
    NSString *subjectS = [NSString stringWithUTF8String:subject];
    NSString *bodyS = [NSString stringWithUTF8String:body];
    if (toS == nil || ccS == nil || bccS == nil || subjectS == nil || bodyS == nil) {
        return 1;
    }
    // return 後は Rust 側の領域が無効になるため、dispatch 前にコピーを取る。
    NSData *attData = nil;
    NSString *attMime = nil;
    NSString *attName = nil;
    if (att_len > 0) {
        attData = [NSData dataWithBytes:att_data length:att_len];
        attMime = [NSString stringWithUTF8String:att_mime];
        attName = [NSString stringWithUTF8String:att_name];
        if (attData == nil || attMime == nil || attName == nil) {
            return 1;
        }
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            BOOL mail = (kind == 0);
            BOOL can = mail ? [MFMailComposeViewController canSendMail]
                            : [MFMessageComposeViewController canSendText];
            if (!can) {
                fandhe_rust_compose_mail_sms_outcome(request_id, 4);
                return;
            }
            UIViewController *top = FandheTopmostViewController();
            if (top == nil || top.view.window == nil) {
                fandhe_rust_compose_mail_sms_outcome(request_id, 5);
                return;
            }
            FandheComposeDelegate *delegate = [FandheComposeDelegate new];
            delegate.requestId = request_id;
            UIViewController *vc = nil;
            if (mail) {
                MFMailComposeViewController *m = [MFMailComposeViewController new];
                [m setToRecipients:FandheSplitRecipients(toS)];
                [m setCcRecipients:FandheSplitRecipients(ccS)];
                [m setBccRecipients:FandheSplitRecipients(bccS)];
                [m setSubject:subjectS];
                [m setMessageBody:bodyS isHTML:NO];
                if (attData != nil) {
                    [m addAttachmentData:attData mimeType:attMime fileName:attName];
                }
                m.mailComposeDelegate = delegate;
                vc = m;
            } else {
                MFMessageComposeViewController *m = [MFMessageComposeViewController new];
                NSArray<NSString *> *to = FandheSplitRecipients(toS);
                if (to.count > 0) {
                    m.recipients = to;
                }
                m.body = bodyS;
                m.messageComposeDelegate = delegate;
                vc = m;
            }
            // スワイプで閉じると delegate が呼ばれず完了が届かないため禁止する。
            vc.modalInPresentation = YES;
            g_retainedDelegate = delegate;
            [top presentViewController:vc animated:YES completion:nil];
            if (top.presentedViewController != vc) {
                g_retainedDelegate = nil;
                delegate.done = YES;
                fandhe_rust_compose_mail_sms_outcome(request_id, 5);
            }
        }
    });
    return 0;
}
