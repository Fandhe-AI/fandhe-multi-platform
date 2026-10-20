// Phase 11 の拡充ループ（親 #108）型サンプル「音楽ライブラリ」: macOS 実装（Issue #482）。
// iTunesLibrary の `ITLibrary` で、端末の曲のタイトルとアーティストを先頭 20 件だけ読む。
// 再生・アートワーク・アルバム・総件数は読まない。曲名・アーティスト・NSError はログに出さない。
//
// `crates/fandhe-egui-proxy/src/device/macos_music_library.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルし、iTunesLibrary を
// リンクする。権限宣言は `macos/Info.plist` の `NSAppleMusicUsageDescription`
// （無いと初回アクセスで OS がプロセスを終了させる。ホストのテスト実行体には Info.plist が
// 埋め込まれないため、Rust のテストから実物の ITLibrary は呼ばない）。
//
// 契約:
//   fandhe_macos_music_library_list(timeout_ms, buf, cap, out_len, out_count, out_overflowed)
//     ワーカースレッドから呼ぶブロッキング関数（main thread では呼ばない）。
//     ITLibrary の読み取りは global queue で行い、呼び出し側は semaphore で `timeout_ms` まで待つ。
//     期限は Rust 側 `LIST_TIMEOUT` の引数だけが決め、ここに固定値は持たない。
//     戻り値: 0 = OK（0 曲も正当な成功）/ 1 = 非対応（ITLibrary が nil。ad-hoc 署名の不足と
//             利用者の拒否はここでは区別できない）/ 2 = 権限拒否（現状この実装では返さない）/
//             3 = 期限切れ / 4 = その他の失敗（多重起動・引数不正を含む）。
//             0 以外のとき out_* と buf は書き換えない。
//     成功時、buf（呼び出し側が確保した cap バイト）へ「title\0artist\0」の組を曲の数だけ連続して詰める。
//     1 要素は 1023 バイトまで（UTF-8 の文字境界で切る）。制御文字は除く（NUL による組のずれ防止）。
//     タグの無い項目は空文字列。out_len は書いた総バイト数、out_count は書いた組の数、
//     21 件目以降の曲がある、または領域が足りなければ *out_overflowed = 1。
//
// メモリ安全: ITLibrary の読み取り block は期限切れの後も走り続けうる。block が書くのは自分の
// `__block` 変数だけで、呼び出し元の buf・out_* には触れない（それらへは semaphore を受けた後に
// 本関数が書く。ios_pedometer.m と同じ約束）。
// 多重起動: 前回の読み取りが走り切っていなければ 4 を返す（期限切れ後の再押下で ITLibrary の処理が
// 積み上がらないようにする。ponytail: プロセス全体で 1 本の旗。複数ライブラリの並行読みは扱わない）。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <Foundation/Foundation.h>
#import <iTunesLibrary/iTunesLibrary.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>

#define FANDHE_TRACK_MAX 20
#define FANDHE_TRACK_FIELD_MAX 1023

static atomic_bool g_in_flight = false;

// 制御文字（NUL を含む）を除いた文字列。nil は空文字列。
static NSString *FandheCleanField(NSString *s) {
    if (s == nil) {
        return @"";
    }
    return [[s componentsSeparatedByCharactersInSet:[NSCharacterSet controlCharacterSet]]
        componentsJoinedByString:@""];
}

int32_t fandhe_macos_music_library_list(uint64_t timeout_ms, uint8_t *buf, size_t cap,
                                        size_t *out_len, uint32_t *out_count,
                                        int32_t *out_overflowed) {
    if (buf == NULL || out_len == NULL || out_count == NULL || out_overflowed == NULL) {
        return 4;
    }
    if (atomic_exchange(&g_in_flight, true)) {
        return 4;
    }
    @autoreleasepool {
        __block int32_t code = 4;
        __block BOOL overflow = NO;
        __block NSMutableArray<NSArray<NSString *> *> *rows = [NSMutableArray array];
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);

        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            @autoreleasepool {
                NSError *err = nil;
                ITLibrary *lib = [ITLibrary libraryWithAPIVersion:@"1.1"
                                                          options:ITLibInitOptionLazyLoadData
                                                            error:&err];
                if (lib == nil) {
                    code = 1;
                } else {
                    for (ITLibMediaItem *item in lib.allMediaItems) {
                        if (item.mediaKind != ITLibMediaItemMediaKindSong) {
                            continue;
                        }
                        if (rows.count == FANDHE_TRACK_MAX) {
                            overflow = YES;
                            break;
                        }
                        [rows addObject:@[ FandheCleanField(item.title),
                                           FandheCleanField(item.artist.name) ]];
                    }
                    [lib unloadData];
                    code = 0;
                }
            }
            atomic_store(&g_in_flight, false);
            dispatch_semaphore_signal(sem);
        });

        dispatch_time_t deadline =
            dispatch_time(DISPATCH_TIME_NOW, (int64_t)(timeout_ms * NSEC_PER_MSEC));
        if (dispatch_semaphore_wait(sem, deadline) != 0) {
            return 3;
        }
        if (code != 0) {
            return code;
        }

        // 呼び出し側スレッド上で buf に詰める（この時点で block は終了済み）。
        size_t pos = 0;
        uint32_t written = 0;
        for (NSArray<NSString *> *row in rows) {
            // 1 組（2 要素 × (最大 1023 バイト + NUL)）が収まらなければ止める。
            if (cap - pos < 2 * (FANDHE_TRACK_FIELD_MAX + 1)) {
                overflow = YES;
                break;
            }
            for (NSString *field in row) {
                NSUInteger used = 0;
                // maxLength 内に収まる文字までを書く（文字の途中では切らない）。
                [field getBytes:buf + pos
                      maxLength:FANDHE_TRACK_FIELD_MAX
                     usedLength:&used
                       encoding:NSUTF8StringEncoding
                        options:0
                          range:NSMakeRange(0, field.length)
                 remainingRange:NULL];
                pos += used;
                buf[pos++] = 0;
            }
            written++;
        }
        *out_len = pos;
        *out_count = written;
        *out_overflowed = overflow ? 1 : 0;
    }
    return 0;
}
