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
//     1 = 点灯、0 = 消灯、-1 = ライト無し、-3 = 失敗（main 以外、または背景遷移での消灯に失敗して
//     まだ光っている間。呼び出し元が `Internal` へ写す）。
//     点灯は `isTorchActive`（実際に光っているか）で読み戻す。消灯の実行・再試行は torchMode が
//     Off かどうかを基準にする（過熱で消えても torchMode が On のまま残るため）。
//
// スレッド: egui の update = main から同期で呼ばれる。main 以外は `dispatch_sync` で main を
// 待たず（デッドロック回避、契約表 11）失敗を返す。set と背景遷移の observer はどちらも
// main で直列に走るのでロックは持たない。AVFoundation 呼び出しは `@autoreleasepool` の内側で行う。
//
// 背景: 初回の呼び出しで `UIApplicationDidEnterBackgroundNotification` の observer を 1 回だけ
// 登録し、点灯中なら消灯して Rust へ再描画を通知する。前景に戻っても点け直さない。
// 登録トークンは static の strong 参照で握り続ける（ARC で解放させない）。
// 背景中の点灯要求は拒否する。背景遷移での消灯に失敗したら失敗状態を保持し、read が -3 を返す。
// set / read の呼び出しごとに消灯を再試行し、消えた（または成功した）時点で状態を解除する。
//
// ログは出さない（NSError を出さない。security.md A09）。プロセス終了時の消灯は OS に任せる。

#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

// Rust 側（`ios_flashlight.rs`）の再描画受け口。
extern void fandhe_rust_flashlight_changed(void);

static id fandhe_flashlight_observer = nil;
// 背景遷移での消灯に失敗し、まだ光っている状態。main でのみ読み書きする。
static BOOL fandhe_flashlight_off_failed = NO;

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

// 消灯失敗の保持を解消する。既に消えていれば解除、光っていれば消灯を再試行する。
static void fandhe_flashlight_retry_off(AVCaptureDevice *d) {
    if (!fandhe_flashlight_off_failed) {
        return;
    }
    if (d.torchMode == AVCaptureTorchModeOff || fandhe_flashlight_apply(d, NO)) {
        fandhe_flashlight_off_failed = NO;
    }
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
                        if (d != nil && d.torchMode != AVCaptureTorchModeOff) {
                            if (!fandhe_flashlight_apply(d, NO)) {
                                fandhe_flashlight_off_failed = YES;
                            }
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
        if (!want) {
            // 消灯は発光状態（isTorchActive）ではなく設定（torchMode）を基準にする。
            // 過熱で消えても torchMode が On のままなら Off へ書き、再点灯経路を断つ。
            fandhe_flashlight_retry_off(d);
            if (d.torchMode == AVCaptureTorchModeOff) {
                return 0;
            }
            return fandhe_flashlight_apply(d, NO) ? 0 : 2;
        }
        if (UIApplication.sharedApplication.applicationState == UIApplicationStateBackground) {
            return 2;
        }
        if (d.torchMode == AVCaptureTorchModeOn && d.isTorchActive) {
            fandhe_flashlight_off_failed = NO;
            return 0;
        }
        if (!d.isTorchAvailable) {
            return 2;
        }
        if (!fandhe_flashlight_apply(d, YES)) {
            return 2;
        }
        // 点灯が確定した要求なので、古い消灯失敗の保持は解除する（read の再試行で消さない）。
        fandhe_flashlight_off_failed = NO;
        return 0;
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
        fandhe_flashlight_retry_off(d);
        if (fandhe_flashlight_off_failed) {
            return -3;
        }
        return d.isTorchActive ? 1 : 0;
    }
}
