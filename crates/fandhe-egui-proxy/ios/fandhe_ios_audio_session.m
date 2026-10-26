// Phase 11 の拡充ループ（親 #172）型サンプル「音声の割り込み・セッション種別」: iOS 実装（Issue #841）。
// 共有の `AVAudioSession` に種別（他の音を止める / 下げる / 混ぜる）を指定して有効にし、止めるまで
// 割り込み・経路変更・メディアサービスのリセットの通知を Rust へ届ける。変更前の category・mode・
// categoryOptions・routeSharingPolicy を保存し、停止・開始途中の失敗の全経路で無効化と復元を行う。
//
// `crates/fandhe-egui-proxy/src/device/ios_audio_session.rs` から C ABI 経由で呼ばれる。
// `build.rs` の iOS 分岐が feature `device` 有効時にのみコンパイルする（AVFoundation はリンク済み）。
//
// 契約（呼ぶのはすべて main thread。`dispatch_sync` は使わない）:
//   fandhe_ios_audio_session_start(id, kind) -> int32_t
//     kind: 0 止める / 1 下げる / 2 混ぜる（`fandhe_device::audio_session::SessionKind::code`）。
//     0 受理 / 1 main 以外 / 2 開始途中の失敗で元へ戻せた / 3 同・戻せなかった / 4 他のセッションが有効 / 5 未知の種別。
//   fandhe_ios_audio_session_stop(id) -> int32_t
//     0 無効化と復元の両方が成功 / 1 どちらかが失敗 / 2 main 以外（後片付けは main へ非同期で送る。結果は不明なので失敗扱い）。
//   fandhe_ios_audio_session_read(category, options, other_playing) -> int32_t
//     0 成功 / 1 失敗（main 以外・NULL）。category: 0 soloAmbient / 1 ambient / 2 playback / 3 record /
//     4 playAndRecord / 5 multiRoute / -1 未知。options は categoryOptions の生のビット。
// 通知は `fandhe_rust_audio_session_signal(id, code)`（コードの表は `fandhe_device::audio_session::signal_from_ios_code`）で
// 任意のスレッドから届く。購読は有効化より先に登録し、有効化の中で同期に届く割り込みも落とさない。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。Rust のポインタは保持しない。`NSError` の内容はログにも出さない。

#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#include <stdint.h>

extern void fandhe_rust_audio_session_signal(uint64_t id, int32_t code);

// 触るのは main thread だけ。
static uint64_t g_active_id = 0;
static AVAudioSessionCategory g_saved_category = nil;
static AVAudioSessionMode g_saved_mode = nil;
static AVAudioSessionCategoryOptions g_saved_options = 0;
static AVAudioSessionRouteSharingPolicy g_saved_policy = AVAudioSessionRouteSharingPolicyDefault;
static NSArray<id> *g_observers = nil;

static void fandhe_remove_observers(void) {
    for (id token in g_observers) {
        [[NSNotificationCenter defaultCenter] removeObserver:token];
    }
    g_observers = nil;
}

// 保存値への復元。成功なら YES。
static BOOL fandhe_restore_category(void) {
    if (g_saved_category == nil) {
        return NO;
    }
    NSError *err = nil;
    return [[AVAudioSession sharedInstance] setCategory:g_saved_category
                                                   mode:g_saved_mode
                                     routeSharingPolicy:g_saved_policy
                                                options:g_saved_options
                                                  error:&err];
}

// 購読解除・無効化・復元を独立に試す。両方成功で YES。状態は結果にかかわらず消す。
static BOOL fandhe_teardown(BOOL deactivate) {
    fandhe_remove_observers();
    BOOL ok = YES;
    if (deactivate) {
        NSError *err = nil;
        ok = [[AVAudioSession sharedInstance]
            setActive:NO
          withOptions:AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation
                error:&err];
    }
    BOOL restored = fandhe_restore_category();
    g_active_id = 0;
    g_saved_category = nil;
    g_saved_mode = nil;
    return ok && restored;
}

static void fandhe_add_observers(uint64_t sid) {
    NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
    AVAudioSession *s = [AVAudioSession sharedInstance];
    id t1 = [nc addObserverForName:AVAudioSessionInterruptionNotification
                            object:s
                             queue:nil
                        usingBlock:^(NSNotification *n) {
        (void)n;
        NSNumber *type = n.userInfo[AVAudioSessionInterruptionTypeKey];
        if (type == nil) {
            return;
        }
        if (type.unsignedIntegerValue == AVAudioSessionInterruptionTypeBegan) {
            fandhe_rust_audio_session_signal(sid, 0);
        } else if (type.unsignedIntegerValue == AVAudioSessionInterruptionTypeEnded) {
            NSNumber *opt = n.userInfo[AVAudioSessionInterruptionOptionKey];
            BOOL resume = opt != nil &&
                (opt.unsignedIntegerValue & AVAudioSessionInterruptionOptionShouldResume) != 0;
            fandhe_rust_audio_session_signal(sid, resume ? 2 : 1);
        }
    }];
    id t2 = [nc addObserverForName:AVAudioSessionRouteChangeNotification
                            object:s
                             queue:nil
                        usingBlock:^(NSNotification *n) {
        (void)n;
        fandhe_rust_audio_session_signal(sid, 3);
    }];
    id t3 = [nc addObserverForName:AVAudioSessionMediaServicesWereResetNotification
                            object:s
                             queue:nil
                        usingBlock:^(NSNotification *n) {
        (void)n;
        fandhe_rust_audio_session_signal(sid, 4);
    }];
    g_observers = @[ t1, t2, t3 ];
}

int32_t fandhe_ios_audio_session_start(uint64_t id, int32_t kind) {
    if (![NSThread isMainThread]) {
        return 1;
    }
    @autoreleasepool {
        if (g_active_id != 0) {
            return 4;
        }
        AVAudioSessionCategoryOptions opts;
        switch (kind) {
            case 0: opts = 0; break;
            case 1: opts = AVAudioSessionCategoryOptionDuckOthers; break;
            case 2: opts = AVAudioSessionCategoryOptionMixWithOthers; break;
            default: return 5;
        }
        AVAudioSession *s = [AVAudioSession sharedInstance];
        g_saved_category = s.category;
        g_saved_mode = s.mode;
        g_saved_options = s.categoryOptions;
        g_saved_policy = s.routeSharingPolicy;
        fandhe_add_observers(id);

        NSError *err = nil;
        BOOL ok = [s setCategory:AVAudioSessionCategoryPlayback
                            mode:AVAudioSessionModeDefault
              routeSharingPolicy:AVAudioSessionRouteSharingPolicyDefault
                         options:opts
                           error:&err];
        if (ok) {
            err = nil;
            ok = [s setActive:YES error:&err];
        }
        if (!ok) {
            // 有効化に失敗した場合は無効化するものが無い。購読解除と category の復元だけ行う。
            return fandhe_teardown(NO) ? 2 : 3;
        }
        g_active_id = id;
        return 0;
    }
}

int32_t fandhe_ios_audio_session_stop(uint64_t id) {
    if (![NSThread isMainThread]) {
        dispatch_async(dispatch_get_main_queue(), ^{
            @autoreleasepool {
                if (g_active_id == id) {
                    fandhe_teardown(YES);
                }
            }
        });
        return 2;
    }
    @autoreleasepool {
        if (g_active_id != id) {
            return 0;
        }
        return fandhe_teardown(YES) ? 0 : 1;
    }
}

int32_t fandhe_ios_audio_session_read(int32_t *category, uint64_t *options, int32_t *other_playing) {
    if (category == NULL || options == NULL || other_playing == NULL || ![NSThread isMainThread]) {
        return 1;
    }
    @autoreleasepool {
        AVAudioSession *s = [AVAudioSession sharedInstance];
        AVAudioSessionCategory c = s.category;
        int32_t code = -1;
        if ([c isEqualToString:AVAudioSessionCategorySoloAmbient]) code = 0;
        else if ([c isEqualToString:AVAudioSessionCategoryAmbient]) code = 1;
        else if ([c isEqualToString:AVAudioSessionCategoryPlayback]) code = 2;
        else if ([c isEqualToString:AVAudioSessionCategoryRecord]) code = 3;
        else if ([c isEqualToString:AVAudioSessionCategoryPlayAndRecord]) code = 4;
        else if ([c isEqualToString:AVAudioSessionCategoryMultiRoute]) code = 5;
        *category = code;
        *options = (uint64_t)s.categoryOptions;
        *other_playing = s.isOtherAudioPlaying ? 1 : 0;
        return 0;
    }
}
