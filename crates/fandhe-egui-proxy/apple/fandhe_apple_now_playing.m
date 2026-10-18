// Phase 11 の拡充ループ（親 #98）型サンプル「ロック画面・OS のメディア操作」: macOS（Issue #381）・iOS（Issue #382）の共用実装。
//
// MediaPlayer の `MPNowPlayingInfoCenter`（曲名・長さ・位置・再生速度）と `MPRemoteCommandCenter`
// （再生・一時停止・切り替え）で、OS の「再生中」表示（macOS はコントロールセンターとメディアキー、iOS はコントロールセンター）へ渡す。
//
// `crates/fandhe-egui-proxy/src/device/apple_now_playing.rs` から C ABI 経由で呼ばれる。
// `build.rs` の macOS・iOS 分岐がコンパイルし MediaPlayer をリンクする。権限・entitlement・usage description は不要。
//
// 契約（戻りコードは固定。NSError の生値・説明は返さずログにも出さない、security.md A09）:
//   int32_t fandhe_apple_now_playing_publish(const uint8_t *title, size_t len, int32_t playing,
//                                            int64_t pos_ms, int64_t dur_ms)
//     0 = 成功、2 = 引数不正（NULL・len 0・UTF-8 でない）、4 = main thread 以外から呼ばれた。
//     title は同期でコピーしポインタを保持しない。pos_ms / dur_ms は負なら未取得として項目を入れない。
//     初回だけ play / pause / togglePlayPause に handler を登録し、トークンを握る。
//     次の曲・前の曲・シーク・スキップ・再生速度は enabled = NO にする（効かない操作 UI を出さない）。
//     macOS は playbackState を毎回設定する（audio session から再生状態を判定できないため。SDK ヘッダの注記）。
//     iOS は playbackState が無く、playbackRate で表す。`AVAudioSession` の category は変えない（前景のみ）。
//   void fandhe_apple_now_playing_clear(void)   冪等。removeTarget: で全トークンを外し、表示を取り下げる。
//     main 以外から呼ばれたら main へ dispatch_async する。要求後に publish が成功していたら（世代が進んだら）
//     古い clear は捨てる（新しい表示を消さない）。
//   void fandhe_apple_now_playing_shutdown(void)  clear と同じ。
// iOS の handler はアプリが Active でないとき Rust を呼ばず CommandFailed を返す（背景で受けた操作に成功を返さない。
// 前景へ戻ると media-playback が中断扱いにし、ドライバが取り下げるので溜まった操作では再開しない）。
// handler は Rust の `fandhe_rust_now_playing_command(int32_t code)`（1 = Play、2 = Pause、3 = Toggle）を呼ぶだけで、
// 戻り値 非 0（受け付けた）なら Success、0（受付停止中）なら CommandFailed を OS へ返す。画面の状態には触れない。
//
// 並行性（device-bridge-contracts.md 11）: 状態は main thread からだけ触る（publish は main 以外を拒否）。
// main への dispatch_sync は使わない（呼び出し元が main のためデッドロックする）。
// 曲名・位置はログへ出力しない。

#import <TargetConditionals.h>
#if TARGET_OS_OSX || TARGET_OS_IOS

#import <Foundation/Foundation.h>
#if TARGET_OS_IOS
#import <UIKit/UIKit.h>
#endif
#import <MediaPlayer/MediaPlayer.h>
#include <stdatomic.h>
#include <stdint.h>

// Rust 側（apple_now_playing.rs）が公開する受け口。非 0 = 受け付けた。
extern int32_t fandhe_rust_now_playing_command(int32_t code);

// 登録済みの (MPRemoteCommand, token) の対。
static NSMutableArray<NSArray *> *g_targets = nil;

// publish 成功ごとに進む世代（main thread だけが書く）。main 以外からの clear が後続の publish を消さないために使う。
static _Atomic uint64_t g_epoch = 0;

static void fandhe_register(MPRemoteCommand *cmd, int32_t code) {
    cmd.enabled = YES;
    id token = [cmd addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *event) {
        (void)event;
#if TARGET_OS_IOS
        if ([UIApplication sharedApplication].applicationState != UIApplicationStateActive) {
            return MPRemoteCommandHandlerStatusCommandFailed;
        }
#endif
        return fandhe_rust_now_playing_command(code) != 0 ? MPRemoteCommandHandlerStatusSuccess
                                                          : MPRemoteCommandHandlerStatusCommandFailed;
    }];
    [g_targets addObject:@[ cmd, token ]];
}

static void fandhe_register_all(void) {
    if (g_targets != nil) {
        return;
    }
    g_targets = [NSMutableArray array];
    MPRemoteCommandCenter *c = [MPRemoteCommandCenter sharedCommandCenter];
    fandhe_register(c.playCommand, 1);
    fandhe_register(c.pauseCommand, 2);
    fandhe_register(c.togglePlayPauseCommand, 3);
    // 効かない操作を OS の UI に出さない。
    c.nextTrackCommand.enabled = NO;
    c.previousTrackCommand.enabled = NO;
    c.seekForwardCommand.enabled = NO;
    c.seekBackwardCommand.enabled = NO;
    c.skipForwardCommand.enabled = NO;
    c.skipBackwardCommand.enabled = NO;
    c.changePlaybackPositionCommand.enabled = NO;
    c.changePlaybackRateCommand.enabled = NO;
}

int32_t fandhe_apple_now_playing_publish(const uint8_t *title, size_t len, int32_t playing,
                                         int64_t pos_ms, int64_t dur_ms) {
    if (![NSThread isMainThread]) {
        return 4;
    }
    if (title == NULL || len == 0) {
        return 2;
    }
    @autoreleasepool {
        NSString *name = [[NSString alloc] initWithBytes:title
                                                  length:len
                                                encoding:NSUTF8StringEncoding];
        if (name == nil) {
            return 2;
        }
        fandhe_register_all();
        NSMutableDictionary *info = [NSMutableDictionary dictionary];
        info[MPMediaItemPropertyTitle] = name;
        info[MPNowPlayingInfoPropertyMediaType] = @(MPNowPlayingInfoMediaTypeAudio);
        info[MPNowPlayingInfoPropertyPlaybackRate] = @(playing ? 1.0 : 0.0);
        if (dur_ms >= 0) {
            info[MPMediaItemPropertyPlaybackDuration] = @((double)dur_ms / 1000.0);
        }
        if (pos_ms >= 0) {
            info[MPNowPlayingInfoPropertyElapsedPlaybackTime] = @((double)pos_ms / 1000.0);
        }
        MPNowPlayingInfoCenter *center = [MPNowPlayingInfoCenter defaultCenter];
        center.nowPlayingInfo = info;
#if TARGET_OS_OSX
        center.playbackState = playing ? MPNowPlayingPlaybackStatePlaying
                                       : MPNowPlayingPlaybackStatePaused;
#endif
    }
    atomic_fetch_add(&g_epoch, 1);
    return 0;
}

// main thread 上で取り下げを実行する。
static void fandhe_clear_on_main(void) {
    if (g_targets == nil) {
        return;  // 何も渡していない（冪等。OS の表示にも触れない）
    }
    @autoreleasepool {
        for (NSArray *pair in g_targets) {
            [(MPRemoteCommand *)pair[0] removeTarget:pair[1]];
        }
        g_targets = nil;
        MPNowPlayingInfoCenter *center = [MPNowPlayingInfoCenter defaultCenter];
        center.nowPlayingInfo = nil;
#if TARGET_OS_OSX
        center.playbackState = MPNowPlayingPlaybackStateStopped;
#endif
    }
}

void fandhe_apple_now_playing_clear(void) {
    if ([NSThread isMainThread]) {
        fandhe_clear_on_main();
        return;
    }
    // dispatch_sync は使わない（main がロックを持つとデッドロックする）。
    uint64_t requested = atomic_load(&g_epoch);
    dispatch_async(dispatch_get_main_queue(), ^{
        if (atomic_load(&g_epoch) == requested) {
            fandhe_clear_on_main();
        }
    });
}

void fandhe_apple_now_playing_shutdown(void) {
    fandhe_apple_now_playing_clear();
}

#endif  // TARGET_OS_OSX || TARGET_OS_IOS
