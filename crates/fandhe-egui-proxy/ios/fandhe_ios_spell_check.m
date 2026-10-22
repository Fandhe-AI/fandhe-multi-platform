// Phase 11 の拡充ループ（親 #119）型サンプル「スペルチェック」: iOS 実装（Issue #579）。
// UIKit の `UITextChecker` で、入力文を OS の辞書で 1 回検査し、誤り語の範囲と修正候補を返す。
// 学習語・無視語（`learnWord:`・`ignoreWord:`・`setIgnoredWords:`）は呼ばない
// （端末のユーザー辞書と共有状態を変えない）。入力文・誤り語・候補はログに出さない。
// 権限・Info.plist・entitlement の宣言は増やさない。
//
// `crates/fandhe-egui-proxy/src/device/ios_spell_check.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルする。
// macOS 版（`macos/fandhe_macos_spell_check.m`）と同じ引数・戻り値コード・バッファ形式で、
// Rust 側の `parse_apple_issue_buffer`・`languages_from_nul_buffer` を共用する。
//
// スレッド: `UITextChecker` は `NS_SWIFT_UI_ACTOR`（main 専用）のため、呼び出しを main queue に集約する。
// main を長く占有しないよう、処理を「1 ステップ = 誤り 1 件の検出と候補取得」に刻み、ステップごとに
// main queue へ dispatch_async し直す。呼び出し元は Rust のワーカースレッド（main では呼ばない）。
//   - 期限はワーカー側の semaphore 待ち（単調時計）で管理する。期限切れで状態を CANCELLED にし、
//     次のステップは何もせず後片付け（UITextChecker の解放）だけして終わる。
//     1 回の OS 呼び出し自体の途中中断はできない（その 1 件分は待つ）。
//   - 結果は job（ヒープ、ARC）に集める。Rust のバッファへは、待ちが成功した後にワーカー側が写す。
//     期限切れ後に届く結果は捨て、Rust のバッファには触れない（use-after-free を防ぐ）。
//   - 完了と取り消しは atomic の状態値で排他にする。ロックを持ったまま待たない。
//
// 守る範囲: 1 プロセス内の要求と完了の順序、嘘の成功の禁止（辞書の無い言語は件数 0 でなく失敗にする）、
// UITextChecker の全経路解放、入力と誤り語のログ非出力。
// 守らない範囲: 期限後も OS 側の検査が走り切ること、利用者が OS に学習させた語による結果の違い。
//
// 契約（戻り値 int32_t: 0 = 成功、それ以外は FANDHE_IOS_SPELL_RC_*）:
//   fandhe_ios_spell_languages(buf, cap, out_len, out_count, timeout_ms)
//     `UITextChecker.availableLanguages` を NUL 区切りの UTF-8 で buf（cap バイト）へ詰める。out_len = 書いたバイト数、
//     out_count = 言語数。収まらなければ切り詰めず RC_BUFFER で失敗にする。
//   fandhe_ios_spell_check(text, text_len, lang, lang_len, max_issues, max_suggestions,
//                            buf, cap, out_len, out_count, out_truncated, timeout_ms)
//     text・lang は長さ付きの UTF-8（NUL 終端を要しない）。誤りを出現順に最大 max_issues 件、
//     1 件ごとに `u32 utf16_start, u32 utf16_len, u32 n`（ネイティブエンディアン）+ 候補 n 個
//     （NUL 終端の UTF-8、最大 max_suggestions 件）を buf へ並べる。max_issues 件を超える誤りが
//     あれば *out_truncated = 1。収まらなければ切り詰めず RC_BUFFER で失敗にする。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <UIKit/UIKit.h>
#import <Foundation/Foundation.h>
#include <dispatch/dispatch.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#define FANDHE_IOS_SPELL_RC_OK 0
#define FANDHE_IOS_SPELL_RC_ARGS 1
#define FANDHE_IOS_SPELL_RC_INVALID_INPUT 2
#define FANDHE_IOS_SPELL_RC_NO_CHECKER 3
#define FANDHE_IOS_SPELL_RC_TIMEOUT 4
#define FANDHE_IOS_SPELL_RC_BUFFER 5
#define FANDHE_IOS_SPELL_RC_RANGE 6
#define FANDHE_IOS_SPELL_RC_FAILED 7

// 候補 1 件の最大バイト数。これを超える候補は捨てる（異常に長い候補への保険）。
#define FANDHE_IOS_SPELL_SUGGESTION_MAX 256

enum { JOB_RUNNING = 0, JOB_FINISHED = 1, JOB_CANCELLED = 2 };

// main queue で走る処理と待つ側が共有する、ヒープ上の作業記録。
@interface FandheIosSpellJob : NSObject {
   @public
    atomic_int state;
}
@property(nonatomic, strong) NSMutableData *data;
@property(nonatomic, assign) int32_t rc;
@property(nonatomic, assign) uint32_t count;
@property(nonatomic, assign) int32_t truncated;
// 全終了経路（完了・取り消し）で main 上で 1 回だけ呼ぶ後片付け。
@property(nonatomic, copy) void (^cleanup)(void);
@end

@implementation FandheIosSpellJob
@end

// step を main queue で刻んで実行し、完了を待つ。step は 1 回の呼び出しで短く終わり、
// 全体が完了したら YES を返す（NO なら main へ再投入される）。
// timeout_ms 以内に完了しなければ取り消して RC_TIMEOUT。block が走り終えた場合のみ job の内容が有効。
static void fandhe_ios_spell_step_on_main(FandheIosSpellJob *job, dispatch_semaphore_t done,
                                      BOOL (^step)(FandheIosSpellJob *)) {
    dispatch_async(dispatch_get_main_queue(), ^{
        if (atomic_load(&job->state) != JOB_RUNNING) {
            if (job.cleanup != nil) {
                job.cleanup();
            }
            return;  // 期限切れで取り消し済み。後片付けだけして終わる。
        }
        if (step(job)) {
            if (job.cleanup != nil) {
                job.cleanup();
            }
            int expected = JOB_RUNNING;
            if (atomic_compare_exchange_strong(&job->state, &expected, JOB_FINISHED)) {
                dispatch_semaphore_signal(done);
            }
        } else {
            fandhe_ios_spell_step_on_main(job, done, step);
        }
    });
}

static int32_t fandhe_ios_spell_run_on_main(int32_t timeout_ms, FandheIosSpellJob *job,
                                        BOOL (^step)(FandheIosSpellJob *)) {
    if (pthread_main_np() != 0) {
        // すでに main 上なら待たずに直接刻まず最後まで呼ぶ。
        while (!step(job)) {
        }
        if (job.cleanup != nil) {
            job.cleanup();
        }
        return job.rc;
    }
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    fandhe_ios_spell_step_on_main(job, done, step);
    int64_t ms = timeout_ms > 0 ? timeout_ms : 0;
    if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, ms * NSEC_PER_MSEC)) == 0) {
        return job.rc;
    }
    int expected = JOB_RUNNING;
    if (atomic_compare_exchange_strong(&job->state, &expected, JOB_CANCELLED)) {
        return FANDHE_IOS_SPELL_RC_TIMEOUT;
    }
    // 取り消す直前に完了していた場合は、その結果を採る。
    if (dispatch_semaphore_wait(done, DISPATCH_TIME_NOW) == 0) {
        return job.rc;
    }
    return FANDHE_IOS_SPELL_RC_TIMEOUT;
}

static void fandhe_ios_spell_append_u32(NSMutableData *d, uint32_t v) {
    [d appendBytes:&v length:sizeof(v)];
}

// job の結果を Rust のバッファへ写す。収まらなければ RC_BUFFER。
static int32_t fandhe_ios_spell_copy_out(FandheIosSpellJob *job, uint8_t *buf, size_t cap, size_t *out_len,
                                     uint32_t *out_count, int32_t *out_truncated) {
    NSUInteger len = job.data.length;
    if (len > cap) {
        return FANDHE_IOS_SPELL_RC_BUFFER;
    }
    if (len > 0) {
        memcpy(buf, job.data.bytes, len);
    }
    *out_len = len;
    *out_count = job.count;
    if (out_truncated != NULL) {
        *out_truncated = job.truncated;
    }
    return FANDHE_IOS_SPELL_RC_OK;
}

int32_t fandhe_ios_spell_languages(uint8_t *buf, size_t cap, size_t *out_len, uint32_t *out_count,
                                     int32_t timeout_ms) {
    if (buf == NULL || out_len == NULL || out_count == NULL) {
        return FANDHE_IOS_SPELL_RC_ARGS;
    }
    @autoreleasepool {
        FandheIosSpellJob *job = [FandheIosSpellJob new];
        job.data = [NSMutableData data];
        int32_t rc = fandhe_ios_spell_run_on_main(timeout_ms, job, ^BOOL(FandheIosSpellJob *j) {
            NSArray<NSString *> *langs = [UITextChecker availableLanguages];
            if (langs == nil) {
                // 件数 0 で成功を返す。Rust 側の言語解決が「辞書なし」の失敗にする。
                j.rc = FANDHE_IOS_SPELL_RC_OK;
                return YES;
            }
            for (NSString *l in langs) {
                const char *u = l.UTF8String;
                if (u == NULL || u[0] == '\0') {
                    continue;
                }
                [j.data appendBytes:u length:strlen(u) + 1];
                j.count += 1;
            }
            j.rc = FANDHE_IOS_SPELL_RC_OK;
            return YES;
        });
        if (rc != FANDHE_IOS_SPELL_RC_OK) {
            return rc;
        }
        return fandhe_ios_spell_copy_out(job, buf, cap, out_len, out_count, NULL);
    }
}

int32_t fandhe_ios_spell_check(const uint8_t *text, size_t text_len, const uint8_t *lang,
                                 size_t lang_len, uint32_t max_issues, uint32_t max_suggestions,
                                 uint8_t *buf, size_t cap, size_t *out_len, uint32_t *out_count,
                                 int32_t *out_truncated, int32_t timeout_ms) {
    if (text == NULL || lang == NULL || buf == NULL || out_len == NULL || out_count == NULL ||
        out_truncated == NULL) {
        return FANDHE_IOS_SPELL_RC_ARGS;
    }
    @autoreleasepool {
        NSString *str = [[NSString alloc] initWithBytes:text
                                                 length:text_len
                                               encoding:NSUTF8StringEncoding];
        NSString *language = [[NSString alloc] initWithBytes:lang
                                                      length:lang_len
                                                    encoding:NSUTF8StringEncoding];
        if (str == nil || language == nil || str.length == 0 || language.length == 0) {
            return FANDHE_IOS_SPELL_RC_INVALID_INPUT;
        }
        FandheIosSpellJob *job = [FandheIosSpellJob new];
        job.data = [NSMutableData data];
        // ステップ間で持ち回る状態（main 上でのみ触る）。
        __block UITextChecker *checker = nil;
        __block NSUInteger pos = 0;
        NSUInteger total = str.length;
        job.cleanup = ^{
            checker = nil;
        };
        int32_t rc = fandhe_ios_spell_run_on_main(timeout_ms, job, ^BOOL(FandheIosSpellJob *j) {
            if (checker == nil) {
                // 呼び出しごとに 1 つ作り、全経路（cleanup）で解放する。
                checker = [[UITextChecker alloc] init];
            }
            if (checker == nil) {
                j.rc = FANDHE_IOS_SPELL_RC_NO_CHECKER;
                return YES;
            }
            if (pos >= total) {
                j.rc = FANDHE_IOS_SPELL_RC_OK;
                return YES;
            }
            // 1 ステップ = 誤り 1 件の検出と候補取得。
            NSRange r = [checker rangeOfMisspelledWordInString:str
                                                         range:NSMakeRange(0, total)
                                                    startingAt:(NSInteger)pos
                                                          wrap:NO
                                                      language:language];
            if (r.location == NSNotFound) {
                j.rc = FANDHE_IOS_SPELL_RC_OK;
                return YES;
            }
            // 長さ 0・後退・範囲外は異常（無限ループ防止）。
            if (r.length == 0 || r.location < pos || r.location + r.length > total) {
                j.rc = FANDHE_IOS_SPELL_RC_RANGE;
                return YES;
            }
            if (j.count >= max_issues) {
                j.truncated = 1;
                j.rc = FANDHE_IOS_SPELL_RC_OK;
                return YES;
            }
            NSArray<NSString *> *guesses = [checker guessesForWordRange:r
                                                              inString:str
                                                              language:language];
            NSMutableData *sugg = [NSMutableData data];
            uint32_t n = 0;
            for (NSString *g in guesses) {
                if (n >= max_suggestions) {
                    break;
                }
                const char *u = g.UTF8String;
                size_t ulen = u != NULL ? strlen(u) : 0;
                if (ulen == 0 || ulen > FANDHE_IOS_SPELL_SUGGESTION_MAX) {
                    continue;
                }
                [sugg appendBytes:u length:ulen + 1];
                n += 1;
            }
            fandhe_ios_spell_append_u32(j.data, (uint32_t)r.location);
            fandhe_ios_spell_append_u32(j.data, (uint32_t)r.length);
            fandhe_ios_spell_append_u32(j.data, n);
            [j.data appendData:sugg];
            j.count += 1;
            pos = r.location + r.length;
            return NO;
        });
        if (rc != FANDHE_IOS_SPELL_RC_OK) {
            return rc;
        }
        return fandhe_ios_spell_copy_out(job, buf, cap, out_len, out_count, out_truncated);
    }
}
