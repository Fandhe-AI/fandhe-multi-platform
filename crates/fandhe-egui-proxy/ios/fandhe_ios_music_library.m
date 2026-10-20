// Phase 11 の拡充ループ（親 #108）型サンプル「音楽ライブラリ」: iOS 実装（Issue #484）。
// MediaPlayer の `MPMediaQuery songsQuery` で、端末の曲のタイトルとアーティストを先頭 20 件だけ読む。
// 再生・アートワーク・アルバム・再生回数・総件数は読まない。曲名・アーティスト・NSError はログに出さない。
//
// `crates/fandhe-egui-proxy/src/device/ios_music_library.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルする。MediaPlayer は
// Now Playing 用にリンク済みで追加リンクは無い。権限宣言は
// `platforms/apple/FandheIosApp/Resources/Info.plist` の `NSAppleMusicUsageDescription`
// （無いと許可要求時に OS がプロセスを終了させる）。macOS 版 `fandhe_macos_music_library.m` と
// 同じ契約で、違いは OS API だけ。
//
// 契約:
//   fandhe_ios_music_library_list(timeout_ms, buf, cap, out_len, out_count, out_overflowed)
//     ワーカースレッドから呼ぶブロッキング関数（main thread では呼ばない）。
//     期限は Rust 側 `LIST_TIMEOUT` の引数だけが決め、起点は本関数の入口。許可ダイアログへの応答待ちと
//     読み取りで同じ期限を共用する。
//     戻り値: 0 = OK（0 曲も正当な成功）/ 1 = 非対応（現状返さない）/ 2 = 権限拒否（Denied・Restricted）/
//             3 = 期限切れ / 4 = その他の失敗（多重起動・引数不正・query 失敗を含む）。
//             0 以外のとき out_* と buf は書き換えない。
//     成功時、buf（呼び出し側が確保した cap バイト）へ「title\0artist\0」の組を曲の数だけ連続して詰める。
//     1 要素は 1023 バイトまで（UTF-8 の文字境界で切る）。制御文字は除く（NUL による組のずれ防止）。
//     タグの無い項目は空文字列。out_len は書いた総バイト数、out_count は書いた組の数、
//     21 件目以降の曲がある、または領域が足りなければ *out_overflowed = 1。
//
// 嘘の成功の回避: 許可が Authorized と確定する前に query しない（拒否中の `items` は空を返すため、
// 0 曲の成功に見えてしまう）。
//
// メモリ安全: 許可要求の handler と読み取り block は期限切れの後も走りうる。どちらも自分の `__block`
// 変数だけに書き、呼び出し元の buf・out_* には触れない（それらへは semaphore を受けた後に本関数が書く）。
// 多重起動: 前回の読み取りが走り切っていなければ 4 を返す（ponytail: プロセス全体で 1 本の旗）。
//
// 再描画: handler は共有状態を書かず、semaphore で同じワーカーへ戻るだけ。画面へ結果を入れるのは
// Rust 側の CompletionSender で、そこで再描画が起こされる。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <Foundation/Foundation.h>
#import <MediaPlayer/MediaPlayer.h>
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

int32_t fandhe_ios_music_library_list(uint64_t timeout_ms, uint8_t *buf, size_t cap,
                                      size_t *out_len, uint32_t *out_count,
                                      int32_t *out_overflowed) {
    if (buf == NULL || out_len == NULL || out_count == NULL || out_overflowed == NULL) {
        return 4;
    }
    @autoreleasepool {
        dispatch_time_t deadline =
            dispatch_time(DISPATCH_TIME_NOW, (int64_t)(timeout_ms * NSEC_PER_MSEC));

        MPMediaLibraryAuthorizationStatus status = [MPMediaLibrary authorizationStatus];
        if (status == MPMediaLibraryAuthorizationStatusNotDetermined) {
            __block MPMediaLibraryAuthorizationStatus granted = status;
            dispatch_semaphore_t auth_sem = dispatch_semaphore_create(0);
            [MPMediaLibrary requestAuthorization:^(MPMediaLibraryAuthorizationStatus s) {
                granted = s;
                dispatch_semaphore_signal(auth_sem);
            }];
            if (dispatch_semaphore_wait(auth_sem, deadline) != 0) {
                return 3;
            }
            status = granted;
        }
        // Denied・Restricted・未知の値はいずれも許可なしとして扱う。
        if (status != MPMediaLibraryAuthorizationStatusAuthorized) {
            return 2;
        }

        if (atomic_exchange(&g_in_flight, true)) {
            return 4;
        }
        __block int32_t code = 4;
        __block BOOL overflow = NO;
        __block NSMutableArray<NSArray<NSString *> *> *rows = [NSMutableArray array];
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);

        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            @autoreleasepool {
                NSArray<MPMediaItem *> *items = [MPMediaQuery songsQuery].items;
                if (items != nil) {
                    for (MPMediaItem *item in items) {
                        if (rows.count == FANDHE_TRACK_MAX) {
                            overflow = YES;
                            break;
                        }
                        [rows addObject:@[ FandheCleanField(item.title),
                                           FandheCleanField(item.artist) ]];
                    }
                    code = 0;
                }
            }
            atomic_store(&g_in_flight, false);
            dispatch_semaphore_signal(sem);
        });

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
