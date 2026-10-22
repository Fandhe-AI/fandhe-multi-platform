// Phase 11 の拡充ループ（親 #119）型サンプル「スペルチェック」: macOS 実装（Issue #578）。
// AppKit の `NSSpellChecker` で、入力文を OS の辞書で 1 回検査し、誤り語の範囲と修正候補を返す。
// 学習語・無視語・共有の言語設定（`learnWord:`・`ignoreWord:`・`setLanguage:`）は呼ばない
// （端末のユーザー辞書と共有状態を変えない）。非推奨の `guessesForWord:` も使わない。
// 入力文・誤り語・候補はログに出さない。権限の宣言は増やさない。
//
// `crates/fandhe-egui-proxy/src/device/macos_spell_check.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// スレッド: `NSSpellChecker` のスレッド安全性を明記した一次情報を確認できていないため、安全側として
// 呼び出しを main queue に集約する。呼び出し元は Rust のワーカースレッド（main では呼ばない）。
//   - 結果は block が所有するヒープ上のオブジェクト（FandheSpellJob、ARC）に集める。Rust のバッファ
//     へは、待ちが成功した（block が走り終えた）後にワーカー側が写す。期限切れの後に block が走っても
//     Rust のバッファには触れない（use-after-free を防ぐ）。
//   - 未開始と取り消しは atomic の状態値で排他にする。期限切れ時に未開始なら取り消し（block は何もしない）、
//     開始済みなら block は走り切る（OS の状態は変えず、document tag も block 内で閉じる）。
//   - ロックを持ったまま待たない。
//
// 契約（戻り値 int32_t: 0 = 成功、それ以外は FANDHE_SPELL_RC_*）:
//   fandhe_macos_spell_languages(buf, cap, out_len, out_count, timeout_ms)
//     `availableLanguages` を NUL 区切りの UTF-8 で buf（cap バイト）へ詰める。out_len = 書いたバイト数、
//     out_count = 言語数。収まらなければ切り詰めず RC_BUFFER で失敗にする。
//   fandhe_macos_spell_check(text, text_len, lang, lang_len, max_issues, max_suggestions,
//                            buf, cap, out_len, out_count, out_truncated, timeout_ms)
//     text・lang は長さ付きの UTF-8（NUL 終端を要しない）。誤りを出現順に最大 max_issues 件、
//     1 件ごとに `u32 utf16_start, u32 utf16_len, u32 n`（ネイティブエンディアン）+ 候補 n 個
//     （NUL 終端の UTF-8、最大 max_suggestions 件）を buf へ並べる。max_issues 件を超える誤りが
//     あれば *out_truncated = 1。収まらなければ切り詰めず RC_BUFFER で失敗にする。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#include <dispatch/dispatch.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#define FANDHE_SPELL_RC_OK 0
#define FANDHE_SPELL_RC_ARGS 1
#define FANDHE_SPELL_RC_INVALID_INPUT 2
#define FANDHE_SPELL_RC_NO_CHECKER 3
#define FANDHE_SPELL_RC_TIMEOUT 4
#define FANDHE_SPELL_RC_BUFFER 5
#define FANDHE_SPELL_RC_RANGE 6
#define FANDHE_SPELL_RC_FAILED 7

// 候補 1 件の最大バイト数。これを超える候補は捨てる（異常に長い候補への保険）。
#define FANDHE_SPELL_SUGGESTION_MAX 256
// 開始済みの block の完了を待つ上限（ミリ秒）。通常は一瞬で終わる。
#define FANDHE_SPELL_STARTED_GRACE_MS 5000

enum { JOB_PENDING = 0, JOB_STARTED = 1, JOB_CANCELLED = 2 };

// main queue で走る処理と待つ側が共有する、ヒープ上の作業記録。
@interface FandheSpellJob : NSObject
@property(nonatomic, strong) NSMutableData *data;
@property(nonatomic, assign) int32_t rc;
@property(nonatomic, assign) uint32_t count;
@property(nonatomic, assign) int32_t truncated;
@end

@implementation FandheSpellJob
@end

// work を main queue で実行して完了を待つ。timeout_ms 以内に開始できなければ取り消して RC_TIMEOUT。
// 成功（RC_OK 以外も含め block が走り終えた）なら job の内容が有効。
static int32_t fandhe_spell_run_on_main(int32_t timeout_ms, FandheSpellJob *job,
                                        void (^work)(FandheSpellJob *)) {
    if (pthread_main_np() != 0) {
        // すでに main 上なら待たずに直接呼ぶ。
        work(job);
        return job.rc;
    }
    __block atomic_int state = JOB_PENDING;
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    dispatch_async(dispatch_get_main_queue(), ^{
        int expected = JOB_PENDING;
        if (!atomic_compare_exchange_strong(&state, &expected, JOB_STARTED)) {
            return;  // 期限切れで取り消し済み。何もしない。
        }
        work(job);
        dispatch_semaphore_signal(done);
    });
    int64_t ms = timeout_ms > 0 ? timeout_ms : 0;
    if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, ms * NSEC_PER_MSEC)) == 0) {
        return job.rc;
    }
    int expected = JOB_PENDING;
    if (atomic_compare_exchange_strong(&state, &expected, JOB_CANCELLED)) {
        return FANDHE_SPELL_RC_TIMEOUT;
    }
    // 開始済み: 走り切るのを短く待つ。間に合わなければ結果を捨てて期限切れにする。
    if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, (int64_t)FANDHE_SPELL_STARTED_GRACE_MS * NSEC_PER_MSEC)) == 0) {
        return job.rc;
    }
    return FANDHE_SPELL_RC_TIMEOUT;
}

static void fandhe_spell_append_u32(NSMutableData *d, uint32_t v) {
    [d appendBytes:&v length:sizeof(v)];
}

// job の結果を Rust のバッファへ写す。収まらなければ RC_BUFFER。
static int32_t fandhe_spell_copy_out(FandheSpellJob *job, uint8_t *buf, size_t cap, size_t *out_len,
                                     uint32_t *out_count, int32_t *out_truncated) {
    NSUInteger len = job.data.length;
    if (len > cap) {
        return FANDHE_SPELL_RC_BUFFER;
    }
    if (len > 0) {
        memcpy(buf, job.data.bytes, len);
    }
    *out_len = len;
    *out_count = job.count;
    if (out_truncated != NULL) {
        *out_truncated = job.truncated;
    }
    return FANDHE_SPELL_RC_OK;
}

int32_t fandhe_macos_spell_languages(uint8_t *buf, size_t cap, size_t *out_len, uint32_t *out_count,
                                     int32_t timeout_ms) {
    if (buf == NULL || out_len == NULL || out_count == NULL) {
        return FANDHE_SPELL_RC_ARGS;
    }
    @autoreleasepool {
        FandheSpellJob *job = [FandheSpellJob new];
        job.data = [NSMutableData data];
        int32_t rc = fandhe_spell_run_on_main(timeout_ms, job, ^(FandheSpellJob *j) {
            NSSpellChecker *sc = [NSSpellChecker sharedSpellChecker];
            if (sc == nil) {
                j.rc = FANDHE_SPELL_RC_NO_CHECKER;
                return;
            }
            NSArray<NSString *> *langs = sc.availableLanguages;
            if (langs == nil) {
                j.rc = FANDHE_SPELL_RC_FAILED;
                return;
            }
            for (NSString *l in langs) {
                const char *u = l.UTF8String;
                if (u == NULL || u[0] == '\0') {
                    continue;
                }
                [j.data appendBytes:u length:strlen(u) + 1];
                j.count += 1;
            }
            j.rc = FANDHE_SPELL_RC_OK;
        });
        if (rc != FANDHE_SPELL_RC_OK) {
            return rc;
        }
        return fandhe_spell_copy_out(job, buf, cap, out_len, out_count, NULL);
    }
}

int32_t fandhe_macos_spell_check(const uint8_t *text, size_t text_len, const uint8_t *lang,
                                 size_t lang_len, uint32_t max_issues, uint32_t max_suggestions,
                                 uint8_t *buf, size_t cap, size_t *out_len, uint32_t *out_count,
                                 int32_t *out_truncated, int32_t timeout_ms) {
    if (text == NULL || lang == NULL || buf == NULL || out_len == NULL || out_count == NULL ||
        out_truncated == NULL) {
        return FANDHE_SPELL_RC_ARGS;
    }
    @autoreleasepool {
        NSString *str = [[NSString alloc] initWithBytes:text
                                                 length:text_len
                                               encoding:NSUTF8StringEncoding];
        NSString *language = [[NSString alloc] initWithBytes:lang
                                                      length:lang_len
                                                    encoding:NSUTF8StringEncoding];
        if (str == nil || language == nil || str.length == 0 || language.length == 0) {
            return FANDHE_SPELL_RC_INVALID_INPUT;
        }
        FandheSpellJob *job = [FandheSpellJob new];
        job.data = [NSMutableData data];
        int32_t rc = fandhe_spell_run_on_main(timeout_ms, job, ^(FandheSpellJob *j) {
            NSSpellChecker *sc = [NSSpellChecker sharedSpellChecker];
            if (sc == nil) {
                j.rc = FANDHE_SPELL_RC_NO_CHECKER;
                return;
            }
            // 呼び出しごとに document tag を取り、全経路で閉じる。
            NSInteger tag = [NSSpellChecker uniqueSpellDocumentTag];
            int32_t result = FANDHE_SPELL_RC_OK;
            NSUInteger pos = 0;
            NSUInteger total = str.length;
            while (pos < total) {
                NSInteger words = 0;
                NSRange r = [sc checkSpellingOfString:str
                                           startingAt:(NSInteger)pos
                                             language:language
                                                 wrap:NO
                               inSpellDocumentWithTag:tag
                                            wordCount:&words];
                if (r.location == NSNotFound) {
                    break;
                }
                // 長さ 0・後退・範囲外は異常（無限ループ防止）。
                if (r.length == 0 || r.location < pos || r.location + r.length > total) {
                    result = FANDHE_SPELL_RC_RANGE;
                    break;
                }
                if (j.count >= max_issues) {
                    j.truncated = 1;
                    break;
                }
                NSArray<NSString *> *guesses = [sc guessesForWordRange:r
                                                              inString:str
                                                              language:language
                                                inSpellDocumentWithTag:tag];
                NSMutableData *sugg = [NSMutableData data];
                uint32_t n = 0;
                for (NSString *g in guesses) {
                    if (n >= max_suggestions) {
                        break;
                    }
                    const char *u = g.UTF8String;
                    size_t ulen = u != NULL ? strlen(u) : 0;
                    if (ulen == 0 || ulen > FANDHE_SPELL_SUGGESTION_MAX) {
                        continue;
                    }
                    [sugg appendBytes:u length:ulen + 1];
                    n += 1;
                }
                fandhe_spell_append_u32(j.data, (uint32_t)r.location);
                fandhe_spell_append_u32(j.data, (uint32_t)r.length);
                fandhe_spell_append_u32(j.data, n);
                [j.data appendData:sugg];
                j.count += 1;
                pos = r.location + r.length;
            }
            [sc closeSpellDocumentWithTag:tag];
            j.rc = result;
        });
        if (rc != FANDHE_SPELL_RC_OK) {
            return rc;
        }
        return fandhe_spell_copy_out(job, buf, cap, out_len, out_count, out_truncated);
    }
}
