// Phase 11 の拡充ループ（親 #1135）型サンプル「画面の録画・キャプチャ」: macOS 実装
// （Issue #1335）。ScreenCaptureKit の SCStream + SCRecordingOutput（macOS 15 以降）で
// 一時 mp4（H.264、マイク音声なし）へ直接書き出す。対象はメインディスプレイ全体の固定
// （ピッカーは使わない）。macOS 15 未満は code=6（Unsupported）で完了通知する。
// iOS 実装（Issue #1336）は `#if TARGET_OS_IPHONE` 側。ReplayKit の `RPScreenRecorder` で
// 自アプリの画面だけを一時 mp4 へ録画する（マイク・カメラなし。アプリ音声は ReplayKit の既定で含まれうる）。
// iOS の結果コード: 1 = 録画が無効（RPRecordingErrorDisabled）、4 = 同意 UI を閉じた・拒否
// （RPRecordingErrorUserDeclined）または同意中に停止、5 = 背景遷移（書き出し済みでも成功にしない。
// `Inactive` のみでは破棄せず 0.5 秒の猶予後に再判定）または OS 起因の中断（error あり）。
// 利用者が OS 側 UI で正常停止した場合（error なし）は明示停止と同じ書き出しを試み、動画が得られれば成功、
// 得られなければ失敗（実機での取得可否は #1334 で確認）。録画時間は停止要求時点までで測る。
// ReplayKit 側の占有は未完了操作の数（g_native_pending）で結果の確定・タイムアウトと分けて管理し、
// 各操作は自分の completion でだけ解放する。0 でない間の新規開始は code=3（busy）で拒否する。
// 破棄停止が上限まで失敗して録画が残る場合は次の開始・停止・背景通知で破棄を再要求する。6 = 6 = `isAvailable == false`・iOS 14 未満・
// シミュレータ（嘘の成功を避け固定で非対応）。
//
// `crates/fandhe-egui-proxy/src/device/apple_screen_capture.rs` から C ABI 経由で呼ばれる。
//
// 契約（pull 型。`fandhe_apple_mic_record.m` と同型）:
//   fandhe_apple_screen_capture_start(uint64_t request_id, double max_seconds) -> int32_t
//     常に 0（依頼を受理し main queue へ積んで即戻る）。結果（busy・非対応を含む）は
//     fandhe_rust_screen_capture_finished で 1 回だけ届く。
//   fandhe_apple_screen_capture_stop(uint64_t request_id) -> int32_t
//     常に 0（受理）。request_id が進行中と一致しない場合は main queue 上で無視する
//     （呼び出し元スレッドで状態を読まない）。
//   fandhe_rust_screen_capture_started(uint64_t request_id)
//     録画が実際に始まった瞬間に 1 回（同意ダイアログ中は呼ばない）。
//   fandhe_rust_screen_capture_finished(uint64_t request_id, int32_t code,
//                                       uint64_t size_bytes, const char *mime,
//                                       uint64_t duration_ms)
//     code: 0 = 成功（size_bytes / mime / duration_ms が有効。mime は戻るまでのみ有効）。
//     1 = 権限拒否（TCC）。2 = 内部エラー。3 = 二重開始（busy）。4 = 同意中に停止
//     （キャンセル）。5 = ライフサイクル破棄（macOS では未使用）。6 = macOS 15 未満。
//     0 以外では mime は NULL。
//
// スレッド: 状態（g_capture_context）への全アクセスを main queue に閉じる。SCK の completion と
// delegate は任意キューから届くので dispatch_async(main) で寄せ、各ブロックは
// @autoreleasepool で囲む。dispatch_sync は使わない（デッドロック回避）。
//
// 確定は FandheFinalize に集約する（first-wins。コンテキストが nil なら何もしない）。全経路で
// 一時ファイルを削除し、SCStream / SCRecordingOutput / delegate の参照を解放する。
// 映像内容・ファイルパスはログへ出さない。

#import <Foundation/Foundation.h>
#import <TargetConditionals.h>
#include <time.h>

extern void fandhe_rust_screen_capture_started(uint64_t request_id);
extern void fandhe_rust_screen_capture_finished(uint64_t request_id, int32_t code,
                                                uint64_t size_bytes, const char *mime,
                                                uint64_t duration_ms);

/// stop 後に完了通知が来ない場合に強制確定するまでの猶予（秒）。画面が「停止中」で止まらない。
static const double kStopGraceSeconds = 5.0;

@interface FandheCaptureContext : NSObject
@property(nonatomic) uint64_t requestId;
@property(nonatomic, strong) NSURL *url;
@property(nonatomic, strong) id stream;    // SCStream
@property(nonatomic, strong) id output;    // SCRecordingOutput
@property(nonatomic, strong) id delegate;  // FandheCaptureDelegate
@property(nonatomic) uint64_t startedNs;  // 単調時計（0 = 未開始）
@property(nonatomic) uint64_t stoppedNs;  // 停止要求の時刻（0 = 未停止）。書き出し待ちを録画時間に含めない
@property(nonatomic) BOOL started;
@property(nonatomic) BOOL stopping;
@property(nonatomic) BOOL interrupted;  // iOS: 背景遷移を受けた
@end
@implementation FandheCaptureContext
@end

static FandheCaptureContext *g_capture_context = nil;  // main queue のみ

static BOOL FandheIsCurrent(FandheCaptureContext *ctx) {
  return ctx != nil && g_capture_context == ctx;
}

/// 一時ファイルのサイズ。新しい NSURL で読みキャッシュ値を避ける。
static uint64_t FandheFileSize(FandheCaptureContext *ctx) {
  NSURL *fresh = [NSURL fileURLWithPath:ctx.url.path];
  NSNumber *size = nil;
  [fresh getResourceValue:&size forKey:NSURLFileSizeKey error:nil];
  return size != nil ? size.unsignedLongLongValue : 0;
}

/// 開始からの実測ミリ秒（1 以上）。
static uint64_t FandheElapsedMs(FandheCaptureContext *ctx) {
  // 壁時計は時刻補正で戻り 0 ms になりうるので単調時計で測る。契約は 0 ms を拒否するため最低 1 ms。
  uint64_t end = ctx.stoppedNs != 0 ? ctx.stoppedNs : clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  uint64_t ms = ctx.startedNs == 0 ? 0 : (end - ctx.startedNs) / 1000000;
  return ms > 0 ? ms : 1;
}

#if TARGET_OS_IPHONE
#import <UIKit/UIKit.h>
#import <ReplayKit/ReplayKit.h>

/// 成功候補が `Inactive` 中に届いたときの猶予（秒）。Control Center 等の一時的な Inactive と
/// 実際のバックグラウンド遷移を見分ける（`fandhe_apple_mic_record.m` の同名定数と同じ値・理由）。
static const double kFandheScreenCaptureInactiveGraceSeconds = 0.5;

/// ReplayKit 側（共有シングルトン）へ発行済みで completion が未着の操作数（main queue のみ）。
/// 結果の確定（g_capture_context）・タイムアウトとは独立で、各操作が自分の completion でだけ
/// 1 つ減らす（他の操作の占有を消さない）。0 より大きい間は新規開始を busy で拒否する（契約表 2・3）。
static int g_native_pending = 0;

static NSURL *FandheNewTempURL(void) {
  NSString *name = [NSString stringWithFormat:@"%@.mp4", [[NSUUID UUID] UUIDString]];
  return [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:name]];
}

/// 破棄停止の再試行上限と間隔。停止が失敗して録画が残る場合に備える（契約表 9）。
static const int kFandheDiscardMaxAttempts = 3;
static const double kFandheDiscardRetrySeconds = 1.0;

static void FandheStopAndDiscardAttempt(int attempt);

/// 録画中のレコーダーを捨て先 URL へ止め、書き出されたファイルを必ず消す（録画インジケータの
/// 残留と画面内容を含む一時ファイルの残留を防ぐ。契約表 3・9）。結果は誰にも通知しない。
/// 操作として 1 つ数え、停止の成功（または再試行上限）まで解放しない。停止が error で
/// 録画が残る場合は上限まで再試行する。上限後も録画が残る場合は FandheRecoverStray が
/// 次の開始・停止・背景通知で破棄を再要求する（回復経路。契約表 9）。
static void FandheStopAndDiscard(void) {
  g_native_pending++;
  FandheStopAndDiscardAttempt(1);
}

/// 管理外の録画（破棄停止が上限まで失敗した等）が残っていれば、操作が飛行中でなく、かつ管理中の
/// セッション（g_capture_context）も無い場合に限り破棄を再要求する。管理中の正常な録画は捨てない。
static void FandheRecoverStray(void) {
  if (g_capture_context == nil && g_native_pending == 0 &&
      [RPScreenRecorder sharedRecorder].isRecording) {
    FandheStopAndDiscard();
  }
}

static void FandheStopAndDiscardAttempt(int attempt) {
  if (@available(iOS 14.0, *)) {
    NSURL *tmp = FandheNewTempURL();
    [[RPScreenRecorder sharedRecorder]
        stopRecordingWithOutputURL:tmp
                 completionHandler:^(NSError *error) {
                   [[NSFileManager defaultManager] removeItemAtURL:tmp error:nil];
                   dispatch_async(dispatch_get_main_queue(), ^{
                     @autoreleasepool {
                       BOOL remains = error != nil && [RPScreenRecorder sharedRecorder].isRecording;
                       if (remains && attempt < kFandheDiscardMaxAttempts) {
                         dispatch_after(
                             dispatch_time(DISPATCH_TIME_NOW,
                                           (int64_t)(kFandheDiscardRetrySeconds * NSEC_PER_SEC)),
                             dispatch_get_main_queue(), ^{
                               @autoreleasepool {
                                 FandheStopAndDiscardAttempt(attempt + 1);
                               }
                             });
                         return;
                       }
                       g_native_pending--;
                     }
                   });
                 }];
  } else {
    g_native_pending--;
  }
}

/// 唯一の確定点。main queue 上でのみ呼ぶ。first-wins（非 current なら何もしない）。
static void FandheFinalize(FandheCaptureContext *ctx, int32_t code, uint64_t size,
                           uint64_t duration_ms) {
  if (!FandheIsCurrent(ctx)) return;
  g_capture_context = nil;
  [[NSFileManager defaultManager] removeItemAtURL:ctx.url error:nil];
  if (code == 0) {
    fandhe_rust_screen_capture_finished(ctx.requestId, 0, size, "video/mp4", duration_ms);
  } else {
    fandhe_rust_screen_capture_finished(ctx.requestId, code, 0, NULL, 0);
  }
}

/// 停止の失敗・猶予切れでの確定。レコーダーが動いたままなら捨てる。
static void FandheFailAndStop(FandheCaptureContext *ctx) {
  if (!FandheIsCurrent(ctx)) return;
  FandheFinalize(ctx, 2, 0, 0);
  // 飛行中の停止があればその completion が後始末する（二重の停止要求を重ねない）。
  FandheRecoverStray();
}

/// 書き出し済みの成功候補を確定する。背景遷移通知と完了の到着順が入れ替わっても、通知ではなく
/// 現在のアプリ状態で判断する（契約表 1）。書き出し済みでも背景遷移なら成功にしない。
static void FandheJudgeSuccess(FandheCaptureContext *ctx, BOOL retried) {
  if (!FandheIsCurrent(ctx)) return;
  UIApplicationState state = [UIApplication sharedApplication].applicationState;
  if (ctx.interrupted || state == UIApplicationStateBackground) {
    FandheFinalize(ctx, 5, 0, 0);
  } else if (state == UIApplicationStateInactive && !retried) {
    dispatch_after(
        dispatch_time(DISPATCH_TIME_NOW,
                      (int64_t)(kFandheScreenCaptureInactiveGraceSeconds * NSEC_PER_SEC)),
        dispatch_get_main_queue(), ^{
          @autoreleasepool {
            FandheJudgeSuccess(ctx, YES);
          }
        });
  } else {
    // 0 バイトは Rust 側の build_capture_result が失敗にする。
    FandheFinalize(ctx, 0, FandheFileSize(ctx), FandheElapsedMs(ctx));
  }
}

static void FandheBeginStop(FandheCaptureContext *ctx) {
  if (!FandheIsCurrent(ctx) || ctx.stopping) return;
  ctx.stopping = YES;
  ctx.stoppedNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
  if (@available(iOS 14.0, *)) {
    g_native_pending++;  // この停止操作を数える。自分の completion で一度だけ解放する
    [[RPScreenRecorder sharedRecorder]
        stopRecordingWithOutputURL:ctx.url
                 completionHandler:^(NSError *error) {
                   dispatch_async(dispatch_get_main_queue(), ^{
                     @autoreleasepool {
                       g_native_pending--;  // この停止操作の完了。他の操作の占有は消さない
                       if (!FandheIsCurrent(ctx)) {
                         // 確定後に届いた書き出し完了。通知は二重にせず、残った一時動画だけ消す。
                         [[NSFileManager defaultManager] removeItemAtURL:ctx.url error:nil];
                         if (error != nil) FandheRecoverStray();  // 停止失敗で録画が残る場合
                         return;
                       }
                       if (error != nil) {
                         FandheFailAndStop(ctx);
                       } else {
                         FandheJudgeSuccess(ctx, NO);
                       }
                     }
                   });
                 }];
    // 完了通知が来ない場合の強制確定（画面を「停止中」のまま止めない。契約表 3）。
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kStopGraceSeconds * NSEC_PER_SEC)),
                   dispatch_get_main_queue(), ^{
                     @autoreleasepool {
                       FandheFailAndStop(ctx);
                     }
                   });
  } else {
    FandheFinalize(ctx, 6, 0, 0);
  }
}

/// OS 側の停止通知（ステータスバーからの停止・systemDormancy・interrupted 等）。
/// error なし（利用者の正常停止）は明示停止と同じ経路で書き出しを試み、得られた動画で成功にする
/// （ファイルが得られない・0 バイトなら失敗。嘘の成功にしない）。error あり（OS 起因の中断）は code=5。
@interface FandheRecorderDelegate : NSObject <RPScreenRecorderDelegate>
@end
@implementation FandheRecorderDelegate
- (void)screenRecorder:(RPScreenRecorder *)screenRecorder
    didStopRecordingWithPreviewViewController:(RPPreviewViewController *)previewViewController
                                        error:(NSError *)error {
  dispatch_async(dispatch_get_main_queue(), ^{
    @autoreleasepool {
      FandheCaptureContext *ctx = g_capture_context;
      if (ctx == nil || !ctx.started || ctx.stopping) return;
      if (error == nil) {
        FandheBeginStop(ctx);
        return;
      }
      FandheFinalize(ctx, 5, 0, 0);
    }
  });
}
@end

/// 背景遷移（main queue）。録画中なら止めて捨て、直ちに code=5 で確定する。同意待ちでも確定し、
/// 遅れて来る開始 completion は非 current として止める。`Inactive` だけでは破棄しない。
static void FandheHandleBackground(void) {
  FandheCaptureContext *ctx = g_capture_context;
  if (ctx == nil) {
    FandheRecoverStray();
    return;
  }
  ctx.interrupted = YES;
  FandheFinalize(ctx, 5, 0, 0);
  FandheRecoverStray();
}

int32_t fandhe_apple_screen_capture_start(uint64_t request_id, double max_seconds) {
  dispatch_async(dispatch_get_main_queue(), ^{
    @autoreleasepool {
      // 通知トークンと delegate は static で強参照する（解放されると登録が無効になる。契約表 1）。
      static id token = nil;
      static FandheRecorderDelegate *recorder_delegate = nil;
      static dispatch_once_t once;
      dispatch_once(&once, ^{
        recorder_delegate = [[FandheRecorderDelegate alloc] init];
        [RPScreenRecorder sharedRecorder].delegate = recorder_delegate;
        token = [[NSNotificationCenter defaultCenter]
            addObserverForName:UIApplicationDidEnterBackgroundNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *note) {
                      (void)note;
                      @autoreleasepool {
                        FandheHandleBackground();
                      }
                    }];
      });
      (void)token;

#if TARGET_OS_SIMULATOR
      (void)max_seconds;
      // シミュレータで ReplayKit が使えるかは Apple の文書に無い。嘘の成功を避けるため固定で非対応。
      fandhe_rust_screen_capture_finished(request_id, 6, 0, NULL, 0);
      return;
#else
      RPScreenRecorder *recorder = [RPScreenRecorder sharedRecorder];
      if (g_capture_context != nil || g_native_pending > 0 || recorder.isRecording) {
        FandheRecoverStray();
        fandhe_rust_screen_capture_finished(request_id, 3, 0, NULL, 0);
        return;
      }
      if (!recorder.isAvailable) {
        fandhe_rust_screen_capture_finished(request_id, 6, 0, NULL, 0);
        return;
      }
      if (@available(iOS 14.0, *)) {
        FandheCaptureContext *ctx = [[FandheCaptureContext alloc] init];
        ctx.requestId = request_id;
        ctx.url = FandheNewTempURL();
        g_capture_context = ctx;
        // 共有シングルトンに残った値を排除し、マイク・カメラは使わない。
        recorder.microphoneEnabled = NO;
        recorder.cameraEnabled = NO;
        g_native_pending++;
        [recorder startRecordingWithHandler:^(NSError *error) {
          dispatch_async(dispatch_get_main_queue(), ^{
            @autoreleasepool {
              // 捨てる場合は破棄操作を数えてから開始操作を閉じる（占有が途切れない）。
              BOOL current = FandheIsCurrent(ctx);
              if (!current && error == nil) FandheStopAndDiscard();
              g_native_pending--;
              if (!current) return;  // 同意中に停止・背景遷移で確定済み
              if (error != nil) {
                int32_t code = 2;
                if (error.code == RPRecordingErrorUserDeclined) code = 4;
                else if (error.code == RPRecordingErrorDisabled) code = 1;
                FandheFinalize(ctx, code, 0, 0);
                return;
              }
              ctx.started = YES;
              ctx.startedNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
              fandhe_rust_screen_capture_started(ctx.requestId);
              // 期限は録画開始から数える（同意ダイアログの待ちを含めない）。
              dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(max_seconds * NSEC_PER_SEC)),
                             dispatch_get_main_queue(), ^{
                               @autoreleasepool {
                                 FandheBeginStop(ctx);
                               }
                             });
            }
          });
        }];
      } else {
        fandhe_rust_screen_capture_finished(request_id, 6, 0, NULL, 0);
      }
#endif
    }
  });
  return 0;
}

int32_t fandhe_apple_screen_capture_stop(uint64_t request_id) {
  dispatch_async(dispatch_get_main_queue(), ^{
    @autoreleasepool {
      FandheCaptureContext *ctx = g_capture_context;
      if (ctx == nil) {
        FandheRecoverStray();
        return;
      }
      if (ctx.requestId != request_id) return;
      if (!ctx.started) {
        // 同意待ち: キャンセルとして確定。遅れて来る開始 completion は非 current として止める。
        FandheFinalize(ctx, 4, 0, 0);
        return;
      }
      FandheBeginStop(ctx);
    }
  });
  return 0;
}

#else
#import <AVFoundation/AVFoundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>

/// delegate の失敗・停止通知から確定するまでの待ち（秒）。この間に recordingOutputDidFinishRecording:
/// が届けば mp4 がファイナライズ済みとして成功確定が先に勝つ（未ファイナライズ mp4 の嘘の成功を避ける）。
static const double kFailureSettleSeconds = 1.0;

/// 唯一の確定点。main queue 上でのみ呼ぶ。
static void FandheFinalize(FandheCaptureContext *ctx, int32_t code, uint64_t size,
                           uint64_t duration_ms) {
  if (!FandheIsCurrent(ctx)) return;
  g_capture_context = nil;
  [[NSFileManager defaultManager] removeItemAtURL:ctx.url error:nil];
  // stream が残っていれば止める（開始直後の stop・録画出力の失敗で動いたままになる経路。契約表 3・9）。
  // 既に停止済みなら error で返るだけ。止まった後に再作成された一時 mp4 も消す。
  if (ctx.stream != nil) {
    if (@available(macOS 15.0, *)) {
      NSURL *url = ctx.url;
      [(SCStream *)ctx.stream stopCaptureWithCompletionHandler:^(NSError *ignored) {
        (void)ignored;
        [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
      }];
    }
  }
  ctx.stream = nil;
  ctx.output = nil;
  ctx.delegate = nil;
  if (code == 0) {
    fandhe_rust_screen_capture_finished(ctx.requestId, 0, size, "video/mp4", duration_ms);
  } else {
    fandhe_rust_screen_capture_finished(ctx.requestId, code, 0, NULL, 0);
  }
}

/// 失敗通知・停止完了待ちタイムアウト時の確定。成功は recordingOutputDidFinishRecording: で
/// 書き出し完了を確認した場合に限る（ファイルサイズだけでは未ファイナライズ mp4 を成功にしてしまう）。
static void FandheFinalizeAfterFailure(FandheCaptureContext *ctx) {
  FandheFinalize(ctx, 2, 0, 0);
}

API_AVAILABLE(macos(15.0))
static void FandheBeginStop(FandheCaptureContext *ctx) {
  if (!FandheIsCurrent(ctx) || ctx.stopping) return;
  ctx.stopping = YES;
  SCStream *stream = ctx.stream;
  [stream stopCaptureWithCompletionHandler:^(NSError *error) {
    if (error == nil) return;  // 通常は recordingOutputDidFinishRecording: が確定する
    // error でも finish コールバックが先着で成功確定することがあるため、猶予後に確定する
    // （FandheFinalize は first-wins。完成した録画を捨てない）。
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kFailureSettleSeconds * NSEC_PER_SEC)),
                   dispatch_get_main_queue(), ^{
                     @autoreleasepool {
                       FandheFinalizeAfterFailure(ctx);
                     }
                   });
  }];
  // 完了通知が来ない場合の強制確定（契約表 3: 画面を「停止中」のまま止めない）。
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kStopGraceSeconds * NSEC_PER_SEC)),
                 dispatch_get_main_queue(), ^{
                   @autoreleasepool {
                     FandheFinalizeAfterFailure(ctx);
                   }
                 });
}

API_AVAILABLE(macos(15.0))
@interface FandheCaptureDelegate : NSObject <SCStreamDelegate, SCRecordingOutputDelegate>
@property(nonatomic, weak) FandheCaptureContext *context;
@property(nonatomic) double maxSeconds;
@end

@implementation FandheCaptureDelegate

- (void)recordingOutputDidStartRecording:(SCRecordingOutput *)recordingOutput {
  FandheCaptureContext *ctx = self.context;
  double max_seconds = self.maxSeconds;
  dispatch_async(dispatch_get_main_queue(), ^{
    @autoreleasepool {
      if (!FandheIsCurrent(ctx) || ctx.started) return;
      ctx.started = YES;
      ctx.startedNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
      fandhe_rust_screen_capture_started(ctx.requestId);
      // 期限は録画開始から数える（同意ダイアログの待ちを含めない）。
      dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(max_seconds * NSEC_PER_SEC)),
                     dispatch_get_main_queue(), ^{
                       @autoreleasepool {
                         if (@available(macOS 15.0, *)) FandheBeginStop(ctx);
                       }
                     });
    }
  });
}

- (void)recordingOutputDidFinishRecording:(SCRecordingOutput *)recordingOutput {
  FandheCaptureContext *ctx = self.context;
  dispatch_async(dispatch_get_main_queue(), ^{
    @autoreleasepool {
      if (!FandheIsCurrent(ctx)) return;
      FandheFinalize(ctx, 0, FandheFileSize(ctx), FandheElapsedMs(ctx));
    }
  });
}

- (void)recordingOutput:(SCRecordingOutput *)recordingOutput didFailWithError:(NSError *)error {
  FandheCaptureContext *ctx = self.context;
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kFailureSettleSeconds * NSEC_PER_SEC)),
                 dispatch_get_main_queue(), ^{
                   @autoreleasepool {
                     FandheFinalizeAfterFailure(ctx);
                   }
                 });
}

// 利用者がメニューバーの停止などで止めた場合も含む。
- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
  FandheCaptureContext *ctx = self.context;
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kFailureSettleSeconds * NSEC_PER_SEC)),
                 dispatch_get_main_queue(), ^{
                   @autoreleasepool {
                     FandheFinalizeAfterFailure(ctx);
                   }
                 });
}

@end

API_AVAILABLE(macos(15.0))
static void FandheStartWithDisplay(FandheCaptureContext *ctx, SCDisplay *display,
                                   double max_seconds) {
  FandheCaptureDelegate *delegate = [[FandheCaptureDelegate alloc] init];
  delegate.context = ctx;
  delegate.maxSeconds = max_seconds;

  SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display
                                                    excludingWindows:@[]];
  SCStreamConfiguration *config = [[SCStreamConfiguration alloc] init];
  config.width = (size_t)display.width;
  config.height = (size_t)display.height;
  config.capturesAudio = NO;

  SCRecordingOutputConfiguration *rec = [[SCRecordingOutputConfiguration alloc] init];
  rec.outputURL = ctx.url;
  rec.outputFileType = AVFileTypeMPEG4;
  rec.videoCodecType = AVVideoCodecTypeH264;
  SCRecordingOutput *output = [[SCRecordingOutput alloc] initWithConfiguration:rec
                                                                      delegate:delegate];
  SCStream *stream = [[SCStream alloc] initWithFilter:filter
                                        configuration:config
                                             delegate:delegate];
  ctx.delegate = delegate;
  ctx.output = output;
  ctx.stream = stream;

  NSError *error = nil;
  if (![stream addRecordingOutput:output error:&error]) {
    FandheFinalize(ctx, 2, 0, 0);
    return;
  }
  [stream startCaptureWithCompletionHandler:^(NSError *startError) {
    if (startError == nil) {
      // 開始は recordingOutputDidStartRecording: で知る。ただし同意ダイアログ待ち中に
      // キャンセル確定済み（非 current）なら、後から開始した stream をここで止める
      // （録画インジケータの残留・削除済みパスへの書き込みを防ぐ。契約表 3・9）。
      dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
          if (!FandheIsCurrent(ctx)) {
            [stream stopCaptureWithCompletionHandler:^(NSError *ignored) {
              (void)ignored;
              // 遅れて開始した SCRecordingOutput が再作成した一時 mp4 を残さない（契約表 9）。
              [[NSFileManager defaultManager] removeItemAtURL:ctx.url error:nil];
            }];
          }
        }
      });
      return;
    }
    int32_t code = startError.code == SCStreamErrorUserDeclined ? 1 : 2;
    dispatch_async(dispatch_get_main_queue(), ^{
      @autoreleasepool {
        // g_capture_context は main queue 専用。現行判定は FandheFinalize 内で行う。
        FandheFinalize(ctx, code, 0, 0);
      }
    });
  }];
}

API_AVAILABLE(macos(15.0))
static void FandheBegin(FandheCaptureContext *ctx, double max_seconds) {
  [SCShareableContent
      getShareableContentExcludingDesktopWindows:NO
                             onScreenWindowsOnly:YES
                               completionHandler:^(SCShareableContent *content, NSError *error) {
                                 dispatch_async(dispatch_get_main_queue(), ^{
                                   @autoreleasepool {
                                     // 待っている間に停止・確定された要求は捨てる。
                                     if (!FandheIsCurrent(ctx) || ctx.stopping) return;
                                     if (error != nil || content == nil) {
                                       FandheFinalize(
                                           ctx, error.code == SCStreamErrorUserDeclined ? 1 : 2,
                                           0, 0);
                                       return;
                                     }
                                     SCDisplay *target = nil;
                                     CGDirectDisplayID main_id = CGMainDisplayID();
                                     for (SCDisplay *d in content.displays) {
                                       if (d.displayID == main_id) {
                                         target = d;
                                         break;
                                       }
                                     }
                                     if (target == nil) {
                                       FandheFinalize(ctx, 2, 0, 0);
                                       return;
                                     }
                                     FandheStartWithDisplay(ctx, target, max_seconds);
                                   }
                                 });
                               }];
}

int32_t fandhe_apple_screen_capture_start(uint64_t request_id, double max_seconds) {
  dispatch_async(dispatch_get_main_queue(), ^{
    @autoreleasepool {
      if (g_capture_context != nil) {
        fandhe_rust_screen_capture_finished(request_id, 3, 0, NULL, 0);
        return;
      }
      if (@available(macOS 15.0, *)) {
        FandheCaptureContext *ctx = [[FandheCaptureContext alloc] init];
        ctx.requestId = request_id;
        NSString *name = [NSString stringWithFormat:@"%@.mp4", [[NSUUID UUID] UUIDString]];
        ctx.url = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:name]];
        g_capture_context = ctx;
        FandheBegin(ctx, max_seconds);
      } else {
        fandhe_rust_screen_capture_finished(request_id, 6, 0, NULL, 0);
      }
    }
  });
  return 0;
}

int32_t fandhe_apple_screen_capture_stop(uint64_t request_id) {
  // request_id の照合は main queue 上の最新状態で行う（呼び出し元スレッドで読まない）。
  dispatch_async(dispatch_get_main_queue(), ^{
    @autoreleasepool {
      FandheCaptureContext *ctx = g_capture_context;
      if (ctx == nil || ctx.requestId != request_id) return;
      if (!ctx.started) {
        // 同意ダイアログ待ち・開始前: キャンセルとして確定。後から届く completion は
        // FandheIsCurrent で捨てられる。
        // 確定が stream を止める。startCapture 完了前なら停止は失敗して無視されるため、
        // 開始 completion が非 current を検知して改めて止める。
        FandheFinalize(ctx, 4, 0, 0);
        return;
      }
      if (@available(macOS 15.0, *)) FandheBeginStop(ctx);
    }
  });
  return 0;
}

#endif  // TARGET_OS_IPHONE
