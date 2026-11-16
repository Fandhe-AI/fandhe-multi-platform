// Phase 11 の拡充ループ（親 #95）「ペン・筆圧入力」: macOS 実装（Issue #355）。
// `NSEvent` の local monitor でマウス系イベント（ペンタブレットはマウスイベントに
// `subtype == NSEventSubtypeTabletPoint` で届く）を見て、種類・筆圧（`pressure`）・傾き（`tilt`）の
// 最新値を static に持つ。イベントは消費も改変もせずそのまま返す（global monitor は使わない）。
// 位置は winit/egui 側から取るため扱わない。値・座標をログへ出さない。
//
// `crates/fandhe-egui-proxy/src/device/macos_pen_stylus.rs` から C ABI 経由で呼ばれる。
// `build.rs` の `build_macos()` が feature `device` 有効時にのみコンパイルする。
//
// 契約:
//   fandhe_macos_pen_stylus_install(void) -> int32_t
//     main thread から呼ぶ。登録はプロセスで 1 回だけ（2 回目以降は 0 を返すだけ）。
//     0 = 登録済み / 1 = 登録失敗（中途半端な登録は残さない）/ 2 = main thread 以外。
//     モニタとオブザーバのトークンは static の strong 参照で握り続ける（即時解放で登録が
//     無効にならないように）。
//   fandhe_macos_pen_stylus_read(FandhePenRaw *out) -> int32_t
//     -1 = main thread 以外 / -2 = 未登録 / -3 = out が NULL / 0 = 接触なし / 1 = 値あり。
//     kind: 0 不明 / 1 ペン / 2 指 / 3 マウス。flags: bit0 = 筆圧あり、bit1 = 傾きあり。
//     tilt_x / tilt_y は `NSEvent.tilt` のまま（-1..1）。度への換算と範囲検証は Rust 側。
//
// 「接触なし」にする契機: マウスアップ、タブレットの近接の離脱、アプリの非アクティブ化。
// static の読み書きはどちらも main thread だけで行うためロックは持たない。
//
// FFI 規約: 全体を `@autoreleasepool` で囲む。ポインタは保持しない。

#import <AppKit/AppKit.h>
#include <stdint.h>
#include <string.h>

typedef struct {
    int32_t kind;
    float pressure;
    float tilt_x;
    float tilt_y;
    uint32_t flags;
} FandhePenRaw;

static BOOL gInstalled = NO;
static id gMonitor = nil;
static id gObserver = nil;
static BOOL gActive = NO;
static FandhePenRaw gRaw;

static void fandhe_pen_clear(void) {
    gActive = NO;
    memset(&gRaw, 0, sizeof(gRaw));
}

static void fandhe_pen_update(NSEvent *event) {
    memset(&gRaw, 0, sizeof(gRaw));
    gActive = YES;
    if (event.subtype == NSEventSubtypeTabletPoint) {
        gRaw.kind = 1;
        gRaw.pressure = event.pressure;
        gRaw.flags |= 1u;
        NSPoint t = event.tilt;
        gRaw.tilt_x = (float)t.x;
        gRaw.tilt_y = (float)t.y;
        gRaw.flags |= 2u;
    } else {
        gRaw.kind = 3;
    }
}

int32_t fandhe_macos_pen_stylus_install(void) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        if (gInstalled) {
            return 0;
        }
        NSEventMask mask = NSEventMaskLeftMouseDown | NSEventMaskLeftMouseDragged |
                           NSEventMaskLeftMouseUp | NSEventMaskTabletProximity;
        id monitor = [NSEvent addLocalMonitorForEventsMatchingMask:mask
                                                           handler:^NSEvent *(NSEvent *event) {
            switch (event.type) {
                case NSEventTypeLeftMouseDown:
                case NSEventTypeLeftMouseDragged:
                    fandhe_pen_update(event);
                    break;
                case NSEventTypeLeftMouseUp:
                    fandhe_pen_clear();
                    break;
                case NSEventTypeTabletProximity:
                    if (!event.isEnteringProximity) {
                        fandhe_pen_clear();
                    }
                    break;
                default:
                    break;
            }
            return event;
        }];
        if (monitor == nil) {
            return 1;
        }
        id observer = [[NSNotificationCenter defaultCenter]
            addObserverForName:NSApplicationDidResignActiveNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *__unused note) {
                        fandhe_pen_clear();
                    }];
        if (observer == nil) {
            [NSEvent removeMonitor:monitor];
            return 1;
        }
        gMonitor = monitor;
        gObserver = observer;
        gInstalled = YES;
        return 0;
    }
}

int32_t fandhe_macos_pen_stylus_read(FandhePenRaw *out) {
    if (![NSThread isMainThread]) {
        return -1;
    }
    if (!gInstalled) {
        return -2;
    }
    if (out == NULL) {
        return -3;
    }
    if (!gActive) {
        return 0;
    }
    *out = gRaw;
    return 1;
}
