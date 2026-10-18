// Phase 11 の拡充ループ（親 #99）型サンプル「外部機器への再生」: macOS 実装（Issue #386）。
// iOS 側は b-ios が TARGET_OS_IOS で足す（このファイルは macOS と共用する前提の置き場）。
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
// 並行性（device-bridge-contracts.md 11）: 状態・世代は 1 本の g_lock の内側で更新する。AppKit の呼び出し
// （addSubview / removeFromSuperview / player の操作）は g_lock を外して行う（delegate が同期で呼ばれても
// デッドロックしない）。delegate は自分の世代を持ち、ロックの内側で g_gen と照合して古い通知を捨てる。
// 呼び出しはすべて egui の update（main thread）から来る。main への dispatch_sync は使わない。
// 権限・entitlement・usage description は不要。出力先の名前はログへ出力しない。

#import <TargetConditionals.h>
#if TARGET_OS_OSX

#import <AVFoundation/AVFoundation.h>
#import <AVKit/AVKit.h>
#import <AppKit/AppKit.h>
#import <CoreAudio/CoreAudio.h>
#import <Foundation/Foundation.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>

enum {
    RC_OK = 0,
    RC_UNSUPPORTED = 1,
    RC_INVALID = 2,
    RC_INTERNAL = 3,
    RC_NOT_MAIN = 4,
    RC_STALE = 6
};

enum { PB_IDLE = 0, PB_READY, PB_PLAYING, PB_STOPPED, PB_FAILED };
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

API_AVAILABLE(macos(10.15))
@interface FandheCastPickerDelegate : NSObject <AVRoutePickerViewDelegate>
@property(nonatomic) uint64_t gen;
@end

@implementation FandheCastPickerDelegate
- (void)routePickerViewWillBeginPresentingRoutes:(AVRoutePickerView *)routePickerView {
    pthread_mutex_lock(&g_lock);
    if (self.gen == g_gen) g_choosing = YES;
    pthread_mutex_unlock(&g_lock);
}
- (void)routePickerViewDidEndPresentingRoutes:(AVRoutePickerView *)routePickerView {
    pthread_mutex_lock(&g_lock);
    if (self.gen == g_gen) g_choosing = NO;
    pthread_mutex_unlock(&g_lock);
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
}

// 既定出力が AirPlay なら 1（name に名前）、そうでなければ 0、読めなければ -1。
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

int32_t fandhe_apple_cast_prepare(uint64_t gen, const uint8_t *wav, size_t len) {
    if (!pthread_main_np()) return RC_NOT_MAIN;
    if (gen == 0 || wav == NULL || len == 0 || len > MAX_WAV_BYTES) return RC_INVALID;
    if (@available(macOS 10.15, *)) {
        @autoreleasepool {
            release_all();
            NSWindow *window = NSApp.mainWindow ?: NSApp.keyWindow ?: NSApp.windows.firstObject;
            NSView *content = window.contentView;
            if (!content) return RC_INTERNAL;
            NSData *data = [NSData dataWithBytes:wav length:len];
            NSError *error = nil;
            AVAudioPlayer *player = [[AVAudioPlayer alloc] initWithData:data error:&error];
            if (!player || ![player prepareToPlay]) return RC_INTERNAL;
            player.numberOfLoops = -1;

            AVRoutePickerView *picker = [[AVRoutePickerView alloc]
                initWithFrame:NSMakeRect(0, 0, PICKER_SIZE, PICKER_SIZE)];
            picker.player = nil;
            FandheCastPickerDelegate *delegate = [[FandheCastPickerDelegate alloc] init];
            delegate.gen = gen;
            picker.delegate = delegate;
            // 窓の右下に固定する（窓のリサイズには追従しない）。flipped かどうかで下端の y が変わる。
            CGFloat y = content.isFlipped ? content.bounds.size.height - PICKER_SIZE - PICKER_MARGIN
                                          : PICKER_MARGIN;
            picker.frame = NSMakeRect(content.bounds.size.width - PICKER_SIZE - PICKER_MARGIN, y,
                                      PICKER_SIZE, PICKER_SIZE);
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
    BOOL ok = [player play];
    pthread_mutex_lock(&g_lock);
    if (gen == g_gen) g_playback = ok ? PB_PLAYING : PB_FAILED;
    pthread_mutex_unlock(&g_lock);
    return ok ? RC_OK : RC_INTERNAL;
}

int32_t fandhe_apple_cast_stop(uint64_t gen) {
    if (!pthread_main_np()) return RC_NOT_MAIN;
    pthread_mutex_lock(&g_lock);
    AVAudioPlayer *player = (gen != 0 && gen == g_gen) ? g_player : nil;
    BOOL playing = g_playback == PB_PLAYING;
    if (player && playing) g_playback = PB_STOPPED;
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
