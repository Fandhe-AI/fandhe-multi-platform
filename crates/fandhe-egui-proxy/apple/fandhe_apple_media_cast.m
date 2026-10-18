// Phase 11 の拡充ループ（親 #99）型サンプル「外部機器への再生」: macOS 実装（Issue #386）と
// iOS 実装（Issue #387。TARGET_OS_IOS の内側で足す。macOS の挙動は変えない）。
//
// 方式（案 A）: 実行時に生成した WAV を AVAudioPlayer（numberOfLoops = -1）でループ再生し、窓の右下に
// AVRoutePickerView（player = nil。システムの出力先を切り替える OS のボタン）を重ねる。
// 経路の状態は snapshot のたびに Core Audio の既定出力デバイスの transport type を読んで決める
// （kAudioDeviceTransportTypeAirPlay なら Connected、名前は kAudioObjectPropertyName）。Core Audio の
// リスナーは登録しない（250ms の pull で足りる。登録トークンの解放漏れの類型を避ける、契約表 1）。
// 名前は OS が返した値だけを使い、読めなければ NotReported（接続中とは言わない、契約表 6）。
// 未確認: AVRoutePickerView の player が nil のときシステムの出力が切り替わるか、AVAudioPlayer が
// 既定出力の切り替えに追従するか（Mac 目視 #379 で確かめる。駄目なら AVPlayer 経路へ切り替える）。
//
// `crates/fandhe-egui-proxy/src/device/apple_media_cast.rs` から C ABI 経由で呼ばれる。
// `build.rs` の macOS 分岐がコンパイルし AVKit・AVFoundation・CoreAudio をリンクする。
//
// 契約（戻りコードは固定。NSError の生値・説明は返さずログにも出さない、security.md A09）:
//   0 = 成功、1 = 非対応（macOS 10.15 未満）、2 = 引数不正（NULL・0 バイト・上限超え）、
//   3 = 内部（WAV を解釈できない・窓・再生を作れない）、4 = main thread 以外から呼ばれた、6 = 世代不一致。
//   int32_t fandhe_apple_cast_prepare(uint64_t gen, const uint8_t *wav, size_t len)
//     gen は Rust が採番する 0 以外の世代。wav は同期でコピーしポインタを保持しない。前の資源を先に解放し、
//     失敗しても作りかけを解放した状態で返す。
//   int32_t fandhe_apple_cast_play(uint64_t gen)   0 / 3 / 4 / 6。
//   int32_t fandhe_apple_cast_stop(uint64_t gen)   0 / 4 / 6。再生中でなければ何もせず 0。
//   void fandhe_apple_cast_snapshot(uint64_t *gen, int32_t *playback, int32_t *route,
//                                   char *name, size_t name_cap)
//     playback: 0 = Idle、1 = Ready、2 = Playing、3 = Stopped、4 = 内部失敗。
//     route: 0 = Local、1 = Choosing、2 = Connected、3 = Disconnected、4 = NotReported。
//     name: route が Connected のとき NUL 終端の UTF-8（取れなければ空）。それ以外は空。
//   void fandhe_apple_cast_release(void)  冪等。
//
// iOS 固有（Issue #387）:
//   - ピッカーは FandheRootViewController().view の右下に固定した AVRoutePickerView。player は設定しない
//     （iOS では AVAudioSession 全体の経路が切り替わる）。
//   - prepare で AVAudioSession の category / mode / options / routeSharingPolicy を一度だけ保存し、
//     playback + longFormAudio にする。setActive:YES は play の直前（開いただけで他アプリの音を止めない）。
//     release_all の末尾で setActive:NO（他アプリへ通知）と保存値の復元を行う。失敗時は保存値を残し次の release で再試行する。
//   - 背景遷移（UIApplicationDidEnterBackgroundNotification）と割り込み開始（Began。AppWasSuspended 理由は無視）で
//     再生中なら pause して playback 5 = Interrupted にする。勝手に再開しない。通知は dispatch_once で一度だけ購読し、
//     トークンは static で持ち続ける（契約表 1）。判断は実際の状態（g_playback）で行うので重複・逆順でも二重に動かない。
//   - 経路は AVAudioSession.currentRoute.outputs に AVAudioSessionPortAirPlay があるときだけ Connected
//     （portName を name に書く。iOS では NotReported を返さない）。route change 通知は再描画だけ起こす。
//   - 状態を OS コールバックで変えた直後、g_lock を外してから fandhe_rust_media_cast_changed() で次フレームを起こす。
//   playback: 5 = Interrupted（iOS のみ）。Interrupted からも play で再開できる。
//   守る範囲: 1 プロセス内の操作順、失敗を成功に畳まないこと、資源と音声セッションの解放。
//   守らない範囲: PiP 等の他画面との AVAudioSession の同時利用、割り込み通知より先に snapshot が読んだ場合の
//   Stopped 表示、OS の UI で切り替えた反映のポーリング間隔ぶんの遅れ、複数プロセス、受信機側の品質。
//
// 並行性（device-bridge-contracts.md 11）: 状態・世代は 1 本の g_lock の内側で更新する。AppKit の呼び出し
// （addSubview / removeFromSuperview / player の操作）は g_lock を外して行う（delegate が同期で呼ばれても
// デッドロックしない）。delegate は自分の世代を持ち、ロックの内側で g_gen と照合して古い通知を捨てる。
// 呼び出しはすべて egui の update（main thread）から来る。main への dispatch_sync は使わない。
// 権限・entitlement・usage description は不要。出力先の名前はログへ出力しない。

#import <TargetConditionals.h>
#if TARGET_OS_OSX || TARGET_OS_IOS

#import <AVFoundation/AVFoundation.h>
#import <AVKit/AVKit.h>
#if TARGET_OS_OSX
#import <AppKit/AppKit.h>
#import <CoreAudio/CoreAudio.h>
#else
#import "fandhe_ios_common.h"
#endif
#import <Foundation/Foundation.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>

#if TARGET_OS_IOS
// ios_media_cast.rs が定義する再描画の受け口。
extern void fandhe_rust_media_cast_changed(void);
#endif

enum {
    RC_OK = 0,
    RC_UNSUPPORTED = 1,
    RC_INVALID = 2,
    RC_INTERNAL = 3,
    RC_NOT_MAIN = 4,
    RC_STALE = 6
};

enum { PB_IDLE = 0, PB_READY, PB_PLAYING, PB_STOPPED, PB_FAILED, PB_INTERRUPTED };
enum { ROUTE_LOCAL = 0, ROUTE_CHOOSING, ROUTE_CONNECTED, ROUTE_DISCONNECTED, ROUTE_NOT_REPORTED };

// 送る WAV の上限（実行時生成の 5 秒・16kHz・16bit モノラルは約 160KB）。
#define MAX_WAV_BYTES (1024 * 1024)
#define PICKER_SIZE 40.0
#define PICKER_MARGIN 8.0

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static uint64_t g_gen = 0;
static int32_t g_playback = PB_IDLE;
static BOOL g_choosing = NO;
// 直前の snapshot で AirPlay に出ていたか。外れたら Disconnected にする。
static BOOL g_was_airplay = NO;
static BOOL g_disconnected = NO;
static AVAudioPlayer *g_player = nil;
static AVRoutePickerView *g_picker = nil;
static id g_delegate = nil;

// OS コールバックが状態を変えた直後に呼ぶ。g_lock を保持したまま呼ばない。macOS は 250ms の pull で足りる。
static void notify_changed(void) {
#if TARGET_OS_IOS
    fandhe_rust_media_cast_changed();
#endif
}

#if TARGET_OS_IOS
// AVAudioSession の保存・適用・復元（契約表 9）。g_audio_lock の内側でだけ触る。
static pthread_mutex_t g_audio_lock = PTHREAD_MUTEX_INITIALIZER;
static BOOL g_audio_saved = NO;
static BOOL g_audio_active = NO;
static AVAudioSessionCategory g_saved_category = nil;
static AVAudioSessionMode g_saved_mode = nil;
static AVAudioSessionCategoryOptions g_saved_options = 0;
static AVAudioSessionRouteSharingPolicy g_saved_policy = AVAudioSessionRouteSharingPolicyDefault;

// 復元に成功したときだけ保存値を捨てる。失敗時は保持し、次の release で再試行する。
static void audio_session_restore(void) {
    pthread_mutex_lock(&g_audio_lock);
    if (g_audio_saved) {
        AVAudioSession *s = [AVAudioSession sharedInstance];
        BOOL ok = YES;
        if (g_audio_active) {
            if ([s setActive:NO
                  withOptions:AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation
                        error:nil]) {
                g_audio_active = NO;
            } else {
                ok = NO;
            }
        }
        if (ok && [s setCategory:g_saved_category
                              mode:g_saved_mode
                routeSharingPolicy:g_saved_policy
                           options:g_saved_options
                             error:nil]) {
            g_audio_saved = NO;
            g_saved_category = nil;
            g_saved_mode = nil;
        }
    }
    pthread_mutex_unlock(&g_audio_lock);
}

// playback + longFormAudio にする（setActive は play 直前）。失敗は NO（保存値は残す）。
static BOOL audio_session_apply(void) {
    pthread_mutex_lock(&g_audio_lock);
    AVAudioSession *s = [AVAudioSession sharedInstance];
    if (!g_audio_saved) {
        g_saved_category = s.category;
        g_saved_mode = s.mode;
        g_saved_options = s.categoryOptions;
        g_saved_policy = s.routeSharingPolicy;
        g_audio_saved = YES;
    }
    BOOL ok = [s setCategory:AVAudioSessionCategoryPlayback
                          mode:AVAudioSessionModeDefault
            routeSharingPolicy:AVAudioSessionRouteSharingPolicyLongFormAudio
                       options:0
                         error:nil];
    pthread_mutex_unlock(&g_audio_lock);
    return ok;
}

static BOOL audio_session_activate(void) {
    pthread_mutex_lock(&g_audio_lock);
    BOOL ok = [[AVAudioSession sharedInstance] setActive:YES error:nil];
    if (ok) g_audio_active = YES;
    pthread_mutex_unlock(&g_audio_lock);
    return ok;
}
#endif

API_AVAILABLE(macos(10.15), ios(13.0))
@interface FandheCastPickerDelegate : NSObject <AVRoutePickerViewDelegate>
@property(nonatomic) uint64_t gen;
@end

@implementation FandheCastPickerDelegate
- (void)routePickerViewWillBeginPresentingRoutes:(AVRoutePickerView *)routePickerView {
    pthread_mutex_lock(&g_lock);
    BOOL changed = self.gen == g_gen;
    if (changed) g_choosing = YES;
    pthread_mutex_unlock(&g_lock);
    if (changed) notify_changed();
}
- (void)routePickerViewDidEndPresentingRoutes:(AVRoutePickerView *)routePickerView {
    pthread_mutex_lock(&g_lock);
    BOOL changed = self.gen == g_gen;
    if (changed) g_choosing = NO;
    pthread_mutex_unlock(&g_lock);
    if (changed) notify_changed();
}
@end

// 資源を取り外して状態を初期化する。AppKit の後始末は g_lock の外で行う。冪等。
static void release_all(void) {
    pthread_mutex_lock(&g_lock);
    AVAudioPlayer *player = g_player;
    AVRoutePickerView *picker = g_picker;
    g_player = nil;
    g_picker = nil;
    g_delegate = nil;
    g_gen = 0;
    g_playback = PB_IDLE;
    g_choosing = NO;
    g_was_airplay = NO;
    g_disconnected = NO;
    pthread_mutex_unlock(&g_lock);
    [player stop];
    if (picker) {
        // view の操作は main thread で行う（終了処理が別スレッドから来ても安全にする）。
        void (^remove)(void) = ^{
            picker.delegate = nil;
            [picker removeFromSuperview];
        };
        if (pthread_main_np()) {
            remove();
        } else {
            dispatch_async(dispatch_get_main_queue(), remove);
        }
    }
#if TARGET_OS_IOS
    audio_session_restore();
#endif
}

// 既定出力が AirPlay なら 1（name に名前）、そうでなければ 0、読めなければ -1（macOS のみ）。
#if TARGET_OS_IOS
static int read_route(char *name, size_t cap) {
    if (name && cap > 0) name[0] = '\0';
    for (AVAudioSessionPortDescription *out in [AVAudioSession sharedInstance].currentRoute.outputs) {
        if (![out.portType isEqualToString:AVAudioSessionPortAirPlay]) continue;
        NSString *s = out.portName;
        NSUInteger used = 0;
        if (name && cap > 0 && s) {
            [s getBytes:name
                 maxLength:cap - 1
                usedLength:&used
                  encoding:NSUTF8StringEncoding
                   options:NSStringEncodingConversionAllowLossy
                     range:NSMakeRange(0, s.length)
            remainingRange:NULL];
            name[used] = '\0';
        }
        return 1;
    }
    return 0;
}

// 再生中なら止めて Interrupted にする。状態で判断するので重複・逆順の通知でも 2 回目は何もしない。
static void interrupt_and_notify(void) {
    pthread_mutex_lock(&g_lock);
    AVAudioPlayer *player = g_playback == PB_PLAYING ? g_player : nil;
    if (player) g_playback = PB_INTERRUPTED;
    pthread_mutex_unlock(&g_lock);
    if (!player) return;
    [player pause];
    notify_changed();
}

// 通知を一度だけ購読する。トークンは static で握り続ける（契約表 1）。
static id g_background_observer = nil;
static id g_interruption_observer = nil;
static id g_route_observer = nil;

static void ensure_observers(void) {
    static dispatch_once_t once_token;
    dispatch_once(&once_token, ^{
        NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
        NSOperationQueue *main = [NSOperationQueue mainQueue];
        g_background_observer = [center addObserverForName:UIApplicationDidEnterBackgroundNotification
                                                    object:nil
                                                     queue:main
                                                usingBlock:^(NSNotification *_Nonnull __unused n) {
                                                  interrupt_and_notify();
                                                }];
        g_interruption_observer = [center
            addObserverForName:AVAudioSessionInterruptionNotification
                        object:nil
                         queue:main
                    usingBlock:^(NSNotification *_Nonnull note) {
                      NSDictionary *info = note.userInfo;
                      // Ended では再開しない。
                      if ([info[AVAudioSessionInterruptionTypeKey] unsignedIntegerValue] !=
                          AVAudioSessionInterruptionTypeBegan) {
                          return;
                      }
                      // アプリ一時停止に由来する遅れた通知は無視する（背景遷移の購読が確定済み）。
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
                      NSNumber *reason = info[AVAudioSessionInterruptionReasonKey];
                      if (reason != nil && reason.unsignedIntegerValue ==
                                               AVAudioSessionInterruptionReasonAppWasSuspended) {
                          return;
                      }
#pragma clang diagnostic pop
                      interrupt_and_notify();
                    }];
        g_route_observer = [center addObserverForName:AVAudioSessionRouteChangeNotification
                                               object:nil
                                                queue:main
                                           usingBlock:^(NSNotification *_Nonnull __unused n) {
                                             notify_changed();
                                           }];
    });
}
#else
static int read_route(char *name, size_t cap) {
    if (name && cap > 0) name[0] = '\0';
    AudioObjectPropertyAddress dev_addr = {kAudioHardwarePropertyDefaultOutputDevice,
                                           kAudioObjectPropertyScopeGlobal,
                                           kAudioObjectPropertyElementMain};
    AudioObjectID device = kAudioObjectUnknown;
    UInt32 size = sizeof(device);
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &dev_addr, 0, NULL, &size, &device) !=
            noErr ||
        device == kAudioObjectUnknown) {
        return -1;
    }
    AudioObjectPropertyAddress type_addr = {kAudioDevicePropertyTransportType,
                                            kAudioObjectPropertyScopeGlobal,
                                            kAudioObjectPropertyElementMain};
    UInt32 transport = 0;
    size = sizeof(transport);
    if (AudioObjectGetPropertyData(device, &type_addr, 0, NULL, &size, &transport) != noErr) {
        return -1;
    }
    if (transport != kAudioDeviceTransportTypeAirPlay) return 0;
    AudioObjectPropertyAddress name_addr = {kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
                                            kAudioObjectPropertyElementMain};
    CFStringRef cf = NULL;
    size = sizeof(cf);
    if (AudioObjectGetPropertyData(device, &name_addr, 0, NULL, &size, &cf) == noErr && cf != NULL) {
        NSString *s = (__bridge_transfer NSString *)cf;
        NSUInteger used = 0;
        if (name && cap > 0) {
            [s getBytes:name
                 maxLength:cap - 1
                usedLength:&used
                  encoding:NSUTF8StringEncoding
                   options:NSStringEncodingConversionAllowLossy
                     range:NSMakeRange(0, s.length)
            remainingRange:NULL];
            name[used] = '\0';
        }
    }
    return 1;
}
#endif

int32_t fandhe_apple_cast_prepare(uint64_t gen, const uint8_t *wav, size_t len) {
    if (!pthread_main_np()) return RC_NOT_MAIN;
    if (gen == 0 || wav == NULL || len == 0 || len > MAX_WAV_BYTES) return RC_INVALID;
    if (@available(macOS 10.15, iOS 13.0, *)) {
        @autoreleasepool {
            release_all();
#if TARGET_OS_IOS
            ensure_observers();
            UIView *content = FandheRootViewController().view;
            if (!content) return RC_INTERNAL;
            if (!audio_session_apply()) {
                audio_session_restore();
                return RC_INTERNAL;
            }
#else
            NSWindow *window = NSApp.mainWindow ?: NSApp.keyWindow ?: NSApp.windows.firstObject;
            NSView *content = window.contentView;
            if (!content) return RC_INTERNAL;
#endif
            NSData *data = [NSData dataWithBytes:wav length:len];
            NSError *error = nil;
            AVAudioPlayer *player = [[AVAudioPlayer alloc] initWithData:data error:&error];
            if (!player || ![player prepareToPlay]) {
#if TARGET_OS_IOS
                audio_session_restore();
#endif
                return RC_INTERNAL;
            }
            player.numberOfLoops = -1;

            AVRoutePickerView *picker = [[AVRoutePickerView alloc]
                initWithFrame:CGRectMake(0, 0, PICKER_SIZE, PICKER_SIZE)];
#if TARGET_OS_OSX
            picker.player = nil;
#endif
            FandheCastPickerDelegate *delegate = [[FandheCastPickerDelegate alloc] init];
            delegate.gen = gen;
            picker.delegate = delegate;
#if TARGET_OS_IOS
            // 右下（セーフエリアの内側）に固定し、回転・リサイズでは左端・上端の余白を伸縮させる。UIKit は上原点。
            UIEdgeInsets inset = content.safeAreaInsets;
            picker.frame = CGRectMake(
                content.bounds.size.width - PICKER_SIZE - PICKER_MARGIN - inset.right,
                content.bounds.size.height - PICKER_SIZE - PICKER_MARGIN - inset.bottom, PICKER_SIZE,
                PICKER_SIZE);
            picker.autoresizingMask =
                UIViewAutoresizingFlexibleLeftMargin | UIViewAutoresizingFlexibleTopMargin;
#else
            // 窓の右下に固定し、リサイズでは左端・上端の余白を伸縮させて右下に追従させる。flipped かどうかで下端の y が変わる。
            CGFloat y = content.isFlipped ? content.bounds.size.height - PICKER_SIZE - PICKER_MARGIN
                                          : PICKER_MARGIN;
            picker.frame = NSMakeRect(content.bounds.size.width - PICKER_SIZE - PICKER_MARGIN, y,
                                      PICKER_SIZE, PICKER_SIZE);
            picker.autoresizingMask = content.isFlipped ? (NSViewMinXMargin | NSViewMinYMargin)
                                                        : (NSViewMinXMargin | NSViewMaxYMargin);
#endif
            [content addSubview:picker];

            pthread_mutex_lock(&g_lock);
            g_player = player;
            g_picker = picker;
            g_delegate = delegate;
            g_gen = gen;
            g_playback = PB_READY;
            pthread_mutex_unlock(&g_lock);
        }
        return RC_OK;
    }
    return RC_UNSUPPORTED;
}

int32_t fandhe_apple_cast_play(uint64_t gen) {
    if (!pthread_main_np()) return RC_NOT_MAIN;
    pthread_mutex_lock(&g_lock);
    AVAudioPlayer *player = (gen != 0 && gen == g_gen) ? g_player : nil;
    pthread_mutex_unlock(&g_lock);
    if (!player) return RC_STALE;
#if TARGET_OS_IOS
    // 開いただけで他アプリの音を止めないよう、有効化は再生の直前に行う。
    BOOL ok = audio_session_activate() && [player play];
#else
    BOOL ok = [player play];
#endif
    pthread_mutex_lock(&g_lock);
    if (gen == g_gen) {
        g_playback = ok ? PB_PLAYING : PB_FAILED;
        g_disconnected = NO;  // 新しい再生開始で切断表示を解く
    }
    pthread_mutex_unlock(&g_lock);
    return ok ? RC_OK : RC_INTERNAL;
}

int32_t fandhe_apple_cast_stop(uint64_t gen) {
    if (!pthread_main_np()) return RC_NOT_MAIN;
    pthread_mutex_lock(&g_lock);
    AVAudioPlayer *player = (gen != 0 && gen == g_gen) ? g_player : nil;
    BOOL playing = g_playback == PB_PLAYING;
    if (player && playing) g_playback = PB_STOPPED;
    if (player) g_disconnected = NO;
    pthread_mutex_unlock(&g_lock);
    if (!player) return RC_STALE;
    if (playing) {
        [player stop];
        player.currentTime = 0;
    }
    return RC_OK;
}

void fandhe_apple_cast_snapshot(uint64_t *gen, int32_t *playback, int32_t *route, char *name,
                                size_t name_cap) {
    if (name && name_cap > 0) name[0] = '\0';
    pthread_mutex_lock(&g_lock);
    AVAudioPlayer *player = g_player;
    *gen = g_gen;
    int32_t pb = g_playback;
    BOOL choosing = g_choosing;
    pthread_mutex_unlock(&g_lock);

    // OS 側で再生が止まった（出力先の切り替え失敗など）場合は成功のままにせず Stopped にする。
    if (pb == PB_PLAYING && player && !player.isPlaying) {
        pthread_mutex_lock(&g_lock);
        if (g_playback == PB_PLAYING && g_player == player) g_playback = PB_STOPPED;
        pb = g_playback;
        pthread_mutex_unlock(&g_lock);
    }
    *playback = pb;
    if (pb == PB_IDLE) {
        *route = ROUTE_LOCAL;
        return;
    }
    if (choosing) {
        *route = ROUTE_CHOOSING;
        return;
    }
    int r = read_route(name, name_cap);
    pthread_mutex_lock(&g_lock);
    if (r == 1) {
        g_was_airplay = YES;
        g_disconnected = NO;
    } else if (r == 0 && g_was_airplay) {
        g_was_airplay = NO;
        g_disconnected = YES;
    }
    BOOL disconnected = g_disconnected;
    pthread_mutex_unlock(&g_lock);
    if (r < 0) {
        *route = ROUTE_NOT_REPORTED;
    } else if (r == 1) {
        *route = ROUTE_CONNECTED;
    } else {
        *route = disconnected ? ROUTE_DISCONNECTED : ROUTE_LOCAL;
    }
}

void fandhe_apple_cast_release(void) {
    @autoreleasepool {
        release_all();
    }
}

#endif
