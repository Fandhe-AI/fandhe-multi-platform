// システム音量の取得: iOS 実装（Issue #786、親 #166）。`AVAudioSession.outputVolume` を読み、
// 音量を変えるための `MPVolumeView`（OS の音量スライダー）を画面の下端に 1 個置く。
//
// `src/device/ios_volume_control.rs` から C ABI 経由で呼ばれる。`build.rs` が iOS 分岐の
// feature `device` 有効時のみ `cc` コンパイルする（AVFoundation・MediaPlayer・UIKit はリンク済み）。
//
// 契約（戻りコードは値渡しの int32_t）:
//   fandhe_ios_volume_control_read(float *out_level) -> int32_t
//     0 = 成功（*out_level に 0.0〜1.0 の値を書く）
//     2 = main スレッド以外から呼ばれた（UIKit・AVAudioSession に触れず返す。`dispatch_sync` はしない）
//     4 = out_level が NULL
//   アプリから音量を設定する公開 API は無い。変更は利用者が `MPVolumeView` かハードウェアボタンで行う。
//
// 音声セッション: category の変更・`setActive`・KVO の登録はしない（契約: 通知は購読せず、
// セッションを変えない）。`outputVolume` は Rust 側が 1 秒ごとに読み直す（pull 型）。
//
// スライダー: 読み取りのたびに main 上で `FandheVolumeEnsureSlider` を呼び、1 個の `MPVolumeView` を
// root view の下端に付ける。scene 接続前など root が取れないときは何もせず、次回の読み取りで再試行する。
// 配置に失敗しても読み取りは成功で返す（ハードウェアボタンで変えられ、画面の案内もボタンに
// 触れているため嘘の成功にならない）。
//
// 取り外し: 画面の離脱を知らせるフックは無く、1 プロセスに画面は 1 つ（`FANDHE_PROXY_APP` は
// ビルド時に選ぶ）。ビューはプロセスの終わりまで残し、OS が片付ける（契約表 11）。
//
// ログは出さない（security.md A09）。

#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <AVFoundation/AVFoundation.h>
#import <MediaPlayer/MediaPlayer.h>
#import "fandhe_ios_common.h"

static MPVolumeView *gVolumeView = nil;

// 音量スライダーを root view の下端に 1 個だけ置く。main スレッドからだけ呼ぶ。
static void FandheVolumeEnsureSlider(void) {
    UIView *root = FandheRootViewController().view;
    if (root == nil) {
        return;
    }
    if (gVolumeView == nil) {
        gVolumeView = [[MPVolumeView alloc] initWithFrame:CGRectZero];
        gVolumeView.translatesAutoresizingMaskIntoConstraints = NO;
    }
    if (gVolumeView.superview == root) {
        return;
    }
    [gVolumeView removeFromSuperview];
    [root addSubview:gVolumeView];
    UILayoutGuide *safe = root.safeAreaLayoutGuide;
    [NSLayoutConstraint activateConstraints:@[
        [gVolumeView.leadingAnchor constraintEqualToAnchor:safe.leadingAnchor constant:16.0],
        [gVolumeView.trailingAnchor constraintEqualToAnchor:safe.trailingAnchor constant:-16.0],
        [gVolumeView.bottomAnchor constraintEqualToAnchor:safe.bottomAnchor],
        [gVolumeView.heightAnchor constraintEqualToConstant:44.0],
    ]];
}

int32_t fandhe_ios_volume_control_read(float *out_level) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    if (out_level == NULL) {
        return 4;
    }
    @autoreleasepool {
        FandheVolumeEnsureSlider();
        *out_level = [AVAudioSession sharedInstance].outputVolume;
        return 0;
    }
}
