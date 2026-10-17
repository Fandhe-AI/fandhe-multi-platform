// ペン・筆圧入力の iOS 実装（Issue #356、親 #95）。winit / egui-winit は iOS でペンか指かと
// `altitudeAngle` を捨てるため、winit の base view へ観測専用の `UIGestureRecognizer` を付けて
// 主ポインタ 1 本の種類・筆圧・傾きを読み、Rust へ渡す。位置は扱わない（egui のポインタから取る）。
//
// `crates/fandhe-egui-proxy/src/device/ios_pen_stylus.rs` から C ABI 経由で呼ばれる。
// `build.rs` が iOS ターゲット・feature `device` 有効時のみ `cc` でコンパイルする。
//
// 契約（`ios_pen_stylus.rs` と対）:
//   int32_t fandhe_ios_pen_stylus_install(void)
//     -> 0 = 取り付け済み（2 回目以降は何もせず 0、冪等）。
//        1 = base view を取得できなかった。
//        2 = main スレッド以外から呼ばれた。
//   fandhe_rust_pen_stylus_set(kind, pressure, altitude_rad, flags)
//     -> kind: 0 不明 / 1 ペン / 2 指 / 3 ポインタ。flags bit0 = 筆圧あり、bit1 = 傾きあり。
//        筆圧はペンで maximumPossibleForce > 0 のときだけ送る（指の 3D Touch は筆圧にしない）。
//        傾きはペンのときだけ送る（指は常に π/2 で嘘の値になる）。
//   fandhe_rust_pen_stylus_clear()
//     -> 追跡中 touch の終了・キャンセル、非アクティブ化、背景遷移で接触なしに戻す。
//
// # ライフサイクル（契約表 1・9）
//
// recognizer・オブザーバのトークンは static の強参照で握り続け、登録はプロセスで 1 回だけ。
// recognizer は touch を奪わない（cancelsTouchesInView / delaysTouches* = NO、同時認識を許可）。
// state は認識へ進めず、追跡中 touch が終わったら Failed にして reset で追跡を消す。
//
// # FFI 境界（契約表 5・11）
//
// 本体は `@autoreleasepool` で囲む。Rust のコールバックは同期ですぐ戻り、`dispatch_sync` は使わない。
// 値・座標をログへ出さない。`NSLog` は使わない。

#import <UIKit/UIKit.h>
#import <UIKit/UIGestureRecognizerSubclass.h>
#import "fandhe_ios_common.h"

extern void fandhe_rust_pen_stylus_set(int32_t kind, float pressure, float altitude_rad, uint32_t flags);
extern void fandhe_rust_pen_stylus_clear(void);

@interface FandhePenRecognizer : UIGestureRecognizer <UIGestureRecognizerDelegate>
- (void)drop;
@end

@implementation FandhePenRecognizer {
    UITouch *_tracked;
}

- (instancetype)init {
    self = [super initWithTarget:nil action:nil];
    if (self) {
        self.cancelsTouchesInView = NO;
        self.delaysTouchesBegan = NO;
        self.delaysTouchesEnded = NO;
        self.delegate = self;
    }
    return self;
}

- (BOOL)gestureRecognizer:(UIGestureRecognizer *)a
    shouldRecognizeSimultaneouslyWithGestureRecognizer:(UIGestureRecognizer *)b {
    return YES;
}

- (void)report:(UITouch *)t {
    int32_t kind = 0;
    switch (t.type) {
        case UITouchTypePencil: kind = 1; break;
        case UITouchTypeDirect: kind = 2; break;
        case UITouchTypeIndirectPointer: kind = 3; break;
        default: break;
    }
    uint32_t flags = 0;
    float pressure = 0, altitude = 0;
    if (kind == 1) {
        if (t.maximumPossibleForce > 0) {
            pressure = (float)(t.force / t.maximumPossibleForce);
            flags |= 1;
        }
        altitude = (float)t.altitudeAngle;
        flags |= 2;
    }
    fandhe_rust_pen_stylus_set(kind, pressure, altitude, flags);
}

- (void)touchesBegan:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [super touchesBegan:touches withEvent:event];
    @autoreleasepool {
        if (_tracked == nil) {
            _tracked = touches.anyObject;
            [self report:_tracked];
        }
    }
}

- (void)touchesMoved:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [super touchesMoved:touches withEvent:event];
    @autoreleasepool {
        if (_tracked != nil && [touches containsObject:_tracked]) {
            [self report:_tracked];
        }
    }
}

- (void)finish:(NSSet<UITouch *> *)touches {
    if (_tracked != nil && [touches containsObject:_tracked]) {
        // reset の到着に依存せず、その場で追跡と最後のサンプルを捨てる。
        _tracked = nil;
        fandhe_rust_pen_stylus_clear();
        self.state = UIGestureRecognizerStateFailed;
    }
}

- (void)touchesEnded:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [super touchesEnded:touches withEvent:event];
    @autoreleasepool { [self finish:touches]; }
}

- (void)touchesCancelled:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [super touchesCancelled:touches withEvent:event];
    @autoreleasepool { [self finish:touches]; }
}

- (void)reset {
    [super reset];
    _tracked = nil;
}

// 非アクティブ化・背景遷移で追跡を捨てる（main queue から呼ぶ）。
- (void)drop {
    _tracked = nil;
    fandhe_rust_pen_stylus_clear();
}

@end

static FandhePenRecognizer *g_recognizer;
static id g_resignToken;
static id g_backgroundToken;

int32_t fandhe_ios_pen_stylus_install(void) {
    @autoreleasepool {
        if (![NSThread isMainThread]) {
            return 2;
        }
        if (g_recognizer != nil) {
            return 0;
        }
        // 他の .m と同じく、presented 先ではなく winit の base view へ正規化する。
        UIView *view = FandheTopmostViewController().view.window.rootViewController.view;
        if (view == nil) {
            return 1;
        }
        FandhePenRecognizer *r = [[FandhePenRecognizer alloc] init];
        [view addGestureRecognizer:r];
        NSNotificationCenter *nc = NSNotificationCenter.defaultCenter;
        void (^drop)(NSNotification *) = ^(NSNotification *__unused n) {
            [r drop];
        };
        g_resignToken = [nc addObserverForName:UIApplicationWillResignActiveNotification
                                        object:nil
                                         queue:NSOperationQueue.mainQueue
                                    usingBlock:drop];
        g_backgroundToken = [nc addObserverForName:UIApplicationDidEnterBackgroundNotification
                                            object:nil
                                             queue:NSOperationQueue.mainQueue
                                        usingBlock:drop];
        g_recognizer = r;
        return 0;
    }
}
