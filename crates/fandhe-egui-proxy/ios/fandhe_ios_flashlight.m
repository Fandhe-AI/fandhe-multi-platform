// ライト（トーチ）: iOS 実装（Issue #460、親 #106）。`AVCaptureDevice` の torch を点け消しする。
//
// `src/device/ios_flashlight.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（AVFoundation / UIKit はリンク済み）。
// コードの意味は `fandhe_device::flashlight` の SSOT（`decode_set` / `decode_read`）に従う。
//
// 契約:
//   fandhe_ios_flashlight_set(int32_t on) -> int32_t
//     0 = 要求どおりの状態（冪等）、1 = ライト無し（シミュレータ・非搭載機）、
//     2 = 失敗（main 以外・背景中の点灯要求・過熱等で使用不可・lock / 設定の失敗）。
//   fandhe_ios_flashlight_read(void) -> int32_t
//     1 = 点灯、0 = 消灯、-1 = ライト無し、-3 = 失敗（main 以外。呼び出し元が `Internal` へ写す）。
//     点灯は `isTorchActive`（実際に光っているか）で読み戻す。
//
// スレッド: egui の update = main から同期で呼ばれる。main 以外は `dispatch_sync` で main を
// 待たず（デッドロック回避、契約表 11）失敗を返す。set と背景遷移の observer はどちらも
// main で直列に走るのでロックは持たない。AVFoundation 呼び出しは `@autoreleasepool` の内側で行う。
//
// 背景: 初回の呼び出しで `UIApplicationDidEnterBackgroundNotification` の observer を 1 回だけ
// 登録し、点灯中なら消灯して Rust へ再描画を通知する。前景に戻っても点け直さない。
// 登録トークンは static の strong 参照で握り続ける（ARC で解放させない）。
// 背景中の点灯要求は拒否する。
//
// ログは出さない（NSError を出さない。security.md A09）。プロセス終了時の消灯は OS に任せる。

#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

// Rust 側（`ios_flashlight.rs`）の再描画受け口。
extern void fandhe_rust_flashlight_changed(void);

static id fandhe_flashlight_observer = nil;

// torch を持つ既定のカメラデバイス。無ければ nil。
static AVCaptureDevice *fandhe_flashlight_device(void) {
    AVCaptureDevice *d = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeVideo];
    if (d == nil || !d.hasTorch || ![d isTorchModeSupported:AVCaptureTorchModeOn]) {
        return nil;
    }
    return d;
}

// lock → 変更 → unlock。lock に成功したら全経路で必ず unlock する。成功なら YES。
static BOOL fandhe_flashlight_apply(AVCaptureDevice *d, BOOL on) {
    NSError *error = nil;
    if (![d lockForConfiguration:&error]) {
        return NO;
    }
    BOOL ok = YES;
    if (on) {
        ok = [d setTorchModeOnWithLevel:AVCaptureMaxAvailableTorchLevel error:&error];
    } else {
        d.torchMode = AVCaptureTorchModeOff;
    }
    [d unlockForConfiguration];
    return ok;
}

static void fandhe_flashlight_ensure_observer(void) {
    if (fandhe_flashlight_observer != nil) {
        return;
    }
    fandhe_flashlight_observer = [[NSNotificationCenter defaultCenter]
        addObserverForName:UIApplicationDidEnterBackgroundNotification
                    object:nil
                     queue:[NSOperationQueue mainQueue]
                usingBlock:^(NSNotification *_Nonnull note __unused) {
                    @autoreleasepool {
                        AVCaptureDevice *d = fandhe_flashlight_device();
                        if (d != nil && d.isTorchActive) {
                            fandhe_flashlight_apply(d, NO);
                        }
                    }
                    fandhe_rust_flashlight_changed();
                }];
}

int32_t fandhe_ios_flashlight_set(int32_t on) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        AVCaptureDevice *d = fandhe_flashlight_device();
        if (d == nil) {
            return 1;
        }
        fandhe_flashlight_ensure_observer();
        BOOL want = (on != 0);
        if (want && UIApplication.sharedApplication.applicationState == UIApplicationStateBackground) {
            return 2;
        }
        if (want == d.isTorchActive) {
            return 0;
        }
        if (want && !d.isTorchAvailable) {
            return 2;
        }
        return fandhe_flashlight_apply(d, want) ? 0 : 2;
    }
}

int32_t fandhe_ios_flashlight_read(void) {
    if (![NSThread isMainThread]) {
        return -3;
    }
    @autoreleasepool {
        AVCaptureDevice *d = fandhe_flashlight_device();
        if (d == nil) {
            return -1;
        }
        fandhe_flashlight_ensure_observer();
        return d.isTorchActive ? 1 : 0;
    }
}
