// Phase 11 の拡充ループ（親 #86）型サンプル「音声の再生」: macOS（Issue #267）・iOS（b-ios）共用の実装。
// `AVAudioPlayer` で渡された WAV バイト列 1 本を再生・一時停止・停止し、状態と位置を返す。
// `AVPlayer` は KVO が要るため使わない（pull 型の snapshot で足りる）。
//
// `crates/fandhe-egui-proxy/src/device/apple_media_playback.rs` から C ABI 経由で呼ばれる。
// `build.rs` の macOS / iOS 分岐がコンパイルする（AVFoundation は既存のリンクで足りる）。
// iOS 固有の部分（背景遷移・割り込み・再描画）は書かない（b-ios が TARGET_OS_IOS 内に足す）。
//
// 契約（戻りコードは固定。NSError の生値・コードは返さない、security.md A09）:
//   int32_t fandhe_apple_media_playback_load(const uint8_t *bytes, size_t len, uint64_t gen)
//     gen は Rust が採番する 0 以外の世代。バイト列は dataWithBytes:length: で同期コピーし、
//     ポインタを保持しない。旧 player は先に解放する（失敗しても解放済みの状態で返す）。
//     0 = 成功、1 = 生成・デコード失敗、2 = 引数不正（NULL・len 0・gen 0）。
//   int32_t fandhe_apple_media_playback_play(uint64_t gen) / _pause / _stop
//     0 = 成功、3 = 世代不一致（player なしを含む）、1 = play が NO。
//     stop は pause して currentTime を 0 に戻す。
//   void fandhe_apple_media_playback_snapshot(uint64_t *gen, int32_t *phase, double *pos, double *dur)
//     phase: 0 = Idle、1 = Ready、2 = Playing、3 = Paused、4 = Ended、5 = Failed(Decode)。
//     pos / dur は秒。Rust 側が NaN・負値を未取得へ写す。
//   void fandhe_apple_media_playback_shutdown(void)  冪等。
//
// 並行性（device-bridge-contracts.md 11）: player・世代・状態は 1 本の g_lock の内側で更新する。
// delegate は自分の世代を持ち、ロックの内側で g_gen と照合して古い世代の通知を捨てる。
// main への dispatch_sync は使わない（呼び出し元が main のためデッドロックする）。
// 再生内容はログへ出力しない。権限・entitlement は不要（再生のみ）。

#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#include <pthread.h>
#include <stdint.h>

enum { PHASE_IDLE = 0, PHASE_READY, PHASE_PLAYING, PHASE_PAUSED, PHASE_ENDED, PHASE_FAILED_DECODE };

@interface FandheAudioDelegate : NSObject <AVAudioPlayerDelegate>
@property(nonatomic) uint64_t gen;
@end

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static AVAudioPlayer *g_player = nil;
static FandheAudioDelegate *g_delegate = nil;
// 現在の世代。0 = player なし。g_lock の内側で更新する。
static uint64_t g_gen = 0;
static int32_t g_phase = PHASE_IDLE;

// g_lock を保持して呼ぶ。player を止めて解放し、Idle に戻す。
static void fandhe_release_locked(void) {
    if (g_player != nil) {
        g_player.delegate = nil;
        [g_player stop];
    }
    g_player = nil;
    g_delegate = nil;
    g_gen = 0;
    g_phase = PHASE_IDLE;
}

@implementation FandheAudioDelegate
- (void)audioPlayerDidFinishPlaying:(AVAudioPlayer *)player successfully:(BOOL)flag {
    pthread_mutex_lock(&g_lock);
    if (self.gen == g_gen && g_phase == PHASE_PLAYING) {
        g_phase = flag ? PHASE_ENDED : PHASE_FAILED_DECODE;
    }
    pthread_mutex_unlock(&g_lock);
}
- (void)audioPlayerDecodeErrorDidOccur:(AVAudioPlayer *)player error:(NSError *)error {
    pthread_mutex_lock(&g_lock);
    if (self.gen == g_gen) {
        g_phase = PHASE_FAILED_DECODE;
    }
    pthread_mutex_unlock(&g_lock);
}
@end

int32_t fandhe_apple_media_playback_load(const uint8_t *bytes, size_t len, uint64_t gen) {
    @autoreleasepool {
        if (bytes == NULL || len == 0 || gen == 0) {
            return 2;
        }
        NSData *data = [NSData dataWithBytes:bytes length:len];
        pthread_mutex_lock(&g_lock);
        fandhe_release_locked();
        NSError *error = nil;
        AVAudioPlayer *player = [[AVAudioPlayer alloc] initWithData:data error:&error];
        if (player == nil || ![player prepareToPlay]) {
            pthread_mutex_unlock(&g_lock);
            return 1;
        }
        FandheAudioDelegate *delegate = [[FandheAudioDelegate alloc] init];
        delegate.gen = gen;
        player.delegate = delegate;
        g_player = player;
        g_delegate = delegate;
        g_gen = gen;
        g_phase = PHASE_READY;
        pthread_mutex_unlock(&g_lock);
        return 0;
    }
}

// op: 0 = play、1 = pause、2 = stop。
static int32_t fandhe_op(uint64_t gen, int op) {
    @autoreleasepool {
        pthread_mutex_lock(&g_lock);
        if (g_player == nil || gen == 0 || gen != g_gen) {
            pthread_mutex_unlock(&g_lock);
            return 3;
        }
        int32_t rc = 0;
        if (op == 0) {
            if ([g_player play]) {
                g_phase = PHASE_PLAYING;
            } else {
                rc = 1;
            }
        } else if (op == 1) {
            [g_player pause];
            if (g_phase == PHASE_PLAYING) {
                g_phase = PHASE_PAUSED;
            }
        } else {
            [g_player pause];
            g_player.currentTime = 0;
            g_phase = PHASE_READY;
        }
        pthread_mutex_unlock(&g_lock);
        return rc;
    }
}

int32_t fandhe_apple_media_playback_play(uint64_t gen) { return fandhe_op(gen, 0); }
int32_t fandhe_apple_media_playback_pause(uint64_t gen) { return fandhe_op(gen, 1); }
int32_t fandhe_apple_media_playback_stop(uint64_t gen) { return fandhe_op(gen, 2); }

void fandhe_apple_media_playback_snapshot(uint64_t *gen, int32_t *phase, double *pos, double *dur) {
    @autoreleasepool {
        pthread_mutex_lock(&g_lock);
        *gen = g_gen;
        *phase = g_phase;
        *pos = g_player != nil ? g_player.currentTime : -1.0;
        *dur = g_player != nil ? g_player.duration : -1.0;
        pthread_mutex_unlock(&g_lock);
    }
}

void fandhe_apple_media_playback_shutdown(void) {
    @autoreleasepool {
        pthread_mutex_lock(&g_lock);
        fandhe_release_locked();
        pthread_mutex_unlock(&g_lock);
    }
}
