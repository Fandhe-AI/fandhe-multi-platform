// Phase 11 の拡充ループ（親 #142）型サンプル「環境音の分類」: macOS 実装（Issue #725）と
// iOS 実装（Issue #726）。iOS / macOS 共用ファイル（`fandhe_apple_speech_recognition.m` と同じ判断）。
// iOS 固有の区画（AVAudioSession、背景・割り込みの購読、再描画の通知）は `#if TARGET_OS_IPHONE`。
// Android / Windows / Web は対応表で「できない」のまま（Rust 側の `UnsupportedSoundClassifier`）。
//
// `crates/fandhe-egui-proxy/src/device/apple_sound_classification.rs` から C ABI 経由で呼ばれる。
//
// 契約（pull 型。OS 側が最新の結果を持ち、Rust 側が間隔つきで読む。`ios_activity_recognition.m` と同型）:
//   int32_t fandhe_apple_sound_classification_start(void)
//     -> 0: 受理（許可待ちを含む。結果は read で読む）/ 1: 非対応（macOS 12 未満 / iOS 15 未満）/
//        2: 権限拒否（同期で分かる場合）/ 3: マイクなし / 9: その他。
//        冪等（許可待ち・聞き取り中の二重呼び出しは 0 を返すだけ）。
//   int32_t fandhe_apple_sound_classification_read(uint64_t *seq, uint8_t *labels /* 3 * 64 */,
//       uint32_t *label_lens /* 3 */, double *confidences /* 3 */, uint32_t *count, int32_t *reason)
//     -> 0: 停止中 / 1: 許可待ち / 2: 聞き取り中（結果なし）/ 3: 結果あり / 4: 中断
//        （reason: 1 背景、2 音声の割り込み、3 入力変更）/ 5: 権限拒否 / 6: マイクなし / 9: 失敗。
//        ラベルは 64 バイトまでコピーし、label_lens には元の UTF-8 長を入れる（64 を超えた
//        ものは Rust 側が捨てる）。ラベル・信頼度の検証は Rust 側が丸めの前に行う。
//        中断・失敗は stop まで同じ値を返す。
//   void fandhe_apple_sound_classification_stop(void)
//     -> 制御キューへ非同期に積んで戻る（停止の中で世代を進めて資源を解放する）。start / read は
//        同じキューへ同期で積むため順序が保たれ、stop より前に積まれた許可の応答・解析結果・通知は
//        世代の不一致で捨てられる。
//
// 並行性（契約表 11）: 状態・世代・結果の格納・許可の応答・通知・start/stop/read は 1 本の制御
// serial queue（g_ctrl）に集める。音声の解析は別の serial queue（g_analysis）で回す。
// どちらのキューも main へ dispatch_sync しない。許可の completion・解析結果・通知は、
// 取り込んだ世代と現在の世代を制御キュー上で比べ、違えば捨てる。
//
// 権限: `AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio` で判定する
// （authorized なら開始、notDetermined なら requestAccess して許可待ち、denied/restricted は拒否）。
//
// 聞き取り: `SNClassifySoundRequest`（SNClassifierIdentifierVersion1、macOS 12 以上）を
// `SNAudioStreamAnalyzer` に登録し、`AVAudioEngine.inputNode` の入力タップのバッファを解析キューへ
// 渡す（リアルタイムの文脈から analyze を直接呼ばない）。入力なしの判定は inputFormat の
// sampleRate / channelCount が 0。
//
// 後始末: FandheSoundTeardown 1 か所に集める。停止・失敗・中断・開始途中の失敗のすべてで通す
// （タップ除去、engine stop、analyzer の removeAllRequests、通知の解除、参照の解放）。
// 入力機器の変更（AVAudioEngineConfigurationChangeNotification）は中断（3）として資源を解放し、
// 自動では再開しない。
//
// iOS（Issue #726）:
// - AVAudioSession: 開始前に category / categoryOptions を保存して Record + active にし、
//   Teardown（全終了経路）で setActive:NO（他アプリへ通知）と保存値への復元を行う。有効化に失敗
//   したら start は失敗（9）で、成功を偽らない。mode は変えない（復元対象を増やさない）。
// - 背景: DidEnterBackground で聞き取り中・許可待ちを中断（1）にして資源を解放する。WillResignActive
//   は見ない（許可ダイアログの表示中は Inactive になるだけで、正常な許可を中断扱いにしないため）。
//   許可の応答が背景遷移の後に届いても、背景では録音を始めず中断（1）にする。自動再開しない。
// - 音声の割り込み: AVAudioSessionInterruptionNotification の Began とメディアサービスのリセットを
//   中断（2）にする。Ended では再開しない。通知トークンは static に持ち Teardown で解除する。
// - 再描画: 非同期の経路で値を確定した直後に fandhe_rust_sound_classification_changed を
//   グローバルキューから呼ぶ（入力がないと egui が描画せず画面が止まるため）。同期の start / read /
//   stop の中では呼ばない（ドライバが間隔つきで再描画を予約している）。
//
// ログ: ラベル・信頼度・NSError を NSLog に出さない（契約表 8、security.md）。
// ワーカーキュー上の Cocoa 呼び出しは @autoreleasepool で囲む（契約表 5）。
// 音声バッファは保存も送信もしない。

#import <Foundation/Foundation.h>
#include <TargetConditionals.h>
#include <stdint.h>
#include <string.h>

#import <AVFoundation/AVFoundation.h>
#import <SoundAnalysis/SoundAnalysis.h>
#if TARGET_OS_IPHONE
#import <UIKit/UIKit.h>

// Rust 側（`ios_sound_classification.rs`）の受け口。`crate::ios::request_repaint` を呼ぶだけ。
extern void fandhe_rust_sound_classification_changed(void);

// 制御キューの外から呼ぶ（main が dispatch_sync(g_ctrl) 中でも入れ子の待ちを作らない）。
#define FANDHE_SOUND_NOTIFY()                                                                  \
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{                  \
        fandhe_rust_sound_classification_changed();                                            \
    })
#else
#define FANDHE_SOUND_NOTIFY() ((void)0)
#endif

enum {
    kSoundIdle = 0,
    kSoundAwaitingPermission = 1,
    kSoundListening = 2,
    kSoundInterrupted = 4,
    kSoundFailed = 9,
};

#define kSoundTopK 3
#define kSoundLabelSlot 64

// 制御キューに閉じる状態。
static dispatch_queue_t g_ctrl;
static dispatch_queue_t g_analysis;
static uint64_t g_generation = 0;
static int g_state = kSoundIdle;
static int32_t g_fail_code = 9;
static int32_t g_interrupt_reason = 0;
static BOOL g_has_result = NO;
// 結果の番号。プロセス内で単調増加し、開始し直しても巻き戻さない。
static uint64_t g_seq = 0;
static uint8_t g_labels[kSoundTopK * kSoundLabelSlot];
static uint32_t g_label_lens[kSoundTopK];
static double g_confidences[kSoundTopK];
static uint32_t g_count = 0;
static AVAudioEngine *g_engine = nil;
static SNAudioStreamAnalyzer *g_analyzer = nil;
static id g_observer = nil;
static id g_config_token = nil;
#if TARGET_OS_IPHONE
static id g_interrupt_token = nil;
static id g_reset_token = nil;
// 背景遷移の購読トークン。プロセスの生存中保持する（ARC で解放されると購読が無効になる）。
static id g_bg_token = nil;
static id g_fg_token = nil;
static BOOL g_background = NO;
// AVAudioSession の開始前の状態（Teardown で復元する）。
static NSString *g_prev_category = nil;
static AVAudioSessionCategoryOptions g_prev_options = 0;
static BOOL g_session_active = NO;
#endif

static void FandheSoundFinish(int state, int32_t code, int32_t reason);

static void FandheSoundEnsureQueues(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        g_ctrl = dispatch_queue_create("com.fandhe.sound.control", DISPATCH_QUEUE_SERIAL);
        g_analysis = dispatch_queue_create("com.fandhe.sound.analysis", DISPATCH_QUEUE_SERIAL);
#if TARGET_OS_IPHONE
        g_bg_token = [[NSNotificationCenter defaultCenter]
            addObserverForName:UIApplicationDidEnterBackgroundNotification
                        object:nil
                         queue:nil
                    usingBlock:^(NSNotification *__unused note) {
                        dispatch_async(g_ctrl, ^{
                            @autoreleasepool {
                                g_background = YES;
                                if (g_state == kSoundListening || g_state == kSoundAwaitingPermission) {
                                    FandheSoundFinish(kSoundInterrupted, 0, 1);
                                    FANDHE_SOUND_NOTIFY();
                                }
                            }
                        });
                    }];
        g_fg_token = [[NSNotificationCenter defaultCenter]
            addObserverForName:UIApplicationWillEnterForegroundNotification
                        object:nil
                         queue:nil
                    usingBlock:^(NSNotification *__unused note) {
                        // 自動では再開しない。
                        dispatch_async(g_ctrl, ^{
                            g_background = NO;
                        });
                    }];
#endif
    });
}

#if TARGET_OS_IPHONE
// AVAudioSession を Record + active にする。失敗時は保存値へ戻して NO。制御キュー上でだけ呼ぶ。
static BOOL FandheSoundActivateSession(void) {
    AVAudioSession *session = [AVAudioSession sharedInstance];
    g_prev_category = session.category;
    g_prev_options = session.categoryOptions;
    NSError *error = nil;
    if (![session setCategory:AVAudioSessionCategoryRecord error:&error] ||
        ![session setActive:YES error:&error]) {
        // 変更前の category / options へ戻す（エラーの内容は出さない）。
        [session setCategory:g_prev_category withOptions:g_prev_options error:nil];
        g_prev_category = nil;
        return NO;
    }
    g_session_active = YES;
    return YES;
}

// 有効化していれば無効にして category / options を戻す（冪等）。制御キュー上でだけ呼ぶ。
static void FandheSoundDeactivateSession(void) {
    if (!g_session_active) {
        return;
    }
    g_session_active = NO;
    AVAudioSession *session = [AVAudioSession sharedInstance];
    [session setActive:NO
           withOptions:AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation
                 error:nil];
    if (g_prev_category != nil) {
        [session setCategory:g_prev_category withOptions:g_prev_options error:nil];
    }
    g_prev_category = nil;
}
#endif

// 資源を全部解放する（冪等）。制御キュー上でだけ呼ぶ。
static void FandheSoundTeardown(void) {
    if (g_config_token != nil) {
        [[NSNotificationCenter defaultCenter] removeObserver:g_config_token];
        g_config_token = nil;
    }
#if TARGET_OS_IPHONE
    if (g_interrupt_token != nil) {
        [[NSNotificationCenter defaultCenter] removeObserver:g_interrupt_token];
        g_interrupt_token = nil;
    }
    if (g_reset_token != nil) {
        [[NSNotificationCenter defaultCenter] removeObserver:g_reset_token];
        g_reset_token = nil;
    }
#endif
    AVAudioEngine *engine = g_engine;
    SNAudioStreamAnalyzer *analyzer = g_analyzer;
    g_engine = nil;
    g_analyzer = nil;
    g_observer = nil;
    if (engine != nil) {
        @try {
            [engine.inputNode removeTapOnBus:0];
            [engine stop];
        } @catch (NSException *e) {
            // 例外の内容は出さない。資源は参照ごと手放す。
        }
    }
    if (analyzer != nil) {
        dispatch_async(g_analysis, ^{
            @autoreleasepool {
                [analyzer removeAllRequests];
            }
        });
    }
#if TARGET_OS_IPHONE
    // タップ除去と engine stop の後にセッションを戻す。全終了経路がここを通る。
    FandheSoundDeactivateSession();
#endif
}

// 状態を確定する。必ず資源を解放してから。制御キュー上でだけ呼ぶ。
static void FandheSoundFinish(int state, int32_t code, int32_t reason) {
    FandheSoundTeardown();
    g_has_result = NO;
    g_count = 0;
    g_state = state;
    g_fail_code = code;
    g_interrupt_reason = reason;
}

@interface FandheSoundObserver : NSObject <SNResultsObserving>
@property(nonatomic, assign) uint64_t generation;
@end

@implementation FandheSoundObserver

- (void)request:(id<SNRequest>)request didProduceResult:(id<SNResult>)result {
    @autoreleasepool {
        if (![result isKindOfClass:[SNClassificationResult class]]) {
            return;
        }
        SNClassificationResult *r = (SNClassificationResult *)result;
        // 先頭の上位 3 件を信頼度の降順で取り出す（並び順の前提に依存しない）。
        NSMutableArray<NSString *> *ids = [NSMutableArray arrayWithCapacity:kSoundTopK];
        NSMutableArray<NSNumber *> *confs = [NSMutableArray arrayWithCapacity:kSoundTopK];
        for (SNClassification *c in r.classifications) {
            double v = c.confidence;
            NSUInteger pos = 0;
            while (pos < confs.count && confs[pos].doubleValue >= v) {
                pos++;
            }
            if (pos >= kSoundTopK) {
                continue;
            }
            [ids insertObject:c.identifier atIndex:pos];
            [confs insertObject:@(v) atIndex:pos];
            if (ids.count > kSoundTopK) {
                [ids removeLastObject];
                [confs removeLastObject];
            }
        }
        uint64_t gen = self.generation;
        dispatch_async(g_ctrl, ^{
            @autoreleasepool {
                if (gen != g_generation || g_state != kSoundListening) {
                    return;
                }
                uint32_t n = 0;
                for (NSUInteger i = 0; i < ids.count && i < kSoundTopK; i++) {
                    const char *utf8 = [ids[i] UTF8String];
                    size_t len = utf8 != NULL ? strlen(utf8) : 0;
                    size_t copy = len < kSoundLabelSlot ? len : kSoundLabelSlot;
                    memset(&g_labels[i * kSoundLabelSlot], 0, kSoundLabelSlot);
                    if (copy > 0) {
                        memcpy(&g_labels[i * kSoundLabelSlot], utf8, copy);
                    }
                    g_label_lens[i] = (uint32_t)len;
                    g_confidences[i] = confs[i].doubleValue;
                    n++;
                }
                g_count = n;
                g_has_result = n > 0;
                if (n > 0) {
                    g_seq++;
                    FANDHE_SOUND_NOTIFY();
                }
            }
        });
    }
}

- (void)request:(id<SNRequest>)request didFailWithError:(NSError *)error {
    // エラーの内容は出さない。
    uint64_t gen = self.generation;
    dispatch_async(g_ctrl, ^{
        @autoreleasepool {
            if (gen == g_generation && g_state == kSoundListening) {
                FandheSoundFinish(kSoundFailed, 9, 0);
                FANDHE_SOUND_NOTIFY();
            }
        }
    });
}

- (void)requestDidComplete:(id<SNRequest>)request {
    // こちらから止めた場合は世代が進んでいて捨てられる。それ以外の完了は失敗として扱う。
    uint64_t gen = self.generation;
    dispatch_async(g_ctrl, ^{
        @autoreleasepool {
            if (gen == g_generation && g_state == kSoundListening) {
                FandheSoundFinish(kSoundFailed, 9, 0);
                FANDHE_SOUND_NOTIFY();
            }
        }
    });
}

@end

// 聞き取りを始める。0 成功 / 1 非対応 / 3 マイクなし / 9 失敗。失敗時は資源を解放済みで戻る。
// 制御キュー上でだけ呼ぶ。
static int32_t FandheSoundBegin(void) {
    if (@available(macOS 12.0, iOS 15.0, *)) {
        @try {
#if TARGET_OS_IPHONE
            // 入力形式はセッションを有効にした後でないと正しく取れない。
            if (!FandheSoundActivateSession()) {
                return 9;
            }
#endif
            AVAudioEngine *engine = [[AVAudioEngine alloc] init];
            AVAudioInputNode *input = engine.inputNode;
            AVAudioFormat *format = [input inputFormatForBus:0];
            if (format == nil || format.sampleRate == 0 || format.channelCount == 0) {
                return 3;
            }
            NSError *error = nil;
            SNClassifySoundRequest *request =
                [[SNClassifySoundRequest alloc] initWithClassifierIdentifier:SNClassifierIdentifierVersion1
                                                                       error:&error];
            if (request == nil) {
                return 9;
            }
            SNAudioStreamAnalyzer *analyzer = [[SNAudioStreamAnalyzer alloc] initWithFormat:format];
            FandheSoundObserver *observer = [[FandheSoundObserver alloc] init];
            observer.generation = g_generation;
            if (![analyzer addRequest:request withObserver:observer error:&error]) {
                return 9;
            }
            g_engine = engine;
            g_analyzer = analyzer;
            g_observer = observer;

            uint64_t gen = g_generation;
            dispatch_queue_t analysisQueue = g_analysis;
            [input installTapOnBus:0
                        bufferSize:8192
                            format:format
                             block:^(AVAudioPCMBuffer *buffer, AVAudioTime *when) {
                                 // リアルタイムの文脈から analyze を直接呼ばず、解析キューへ渡す。
                                 AVAudioFramePosition pos = when.sampleTime;
                                 dispatch_async(analysisQueue, ^{
                                     @autoreleasepool {
                                         @try {
                                             [analyzer analyzeAudioBuffer:buffer atAudioFramePosition:pos];
                                         } @catch (NSException *e) {
                                             // 内容は出さない。制御キューへ戻し、世代と状態を確かめて
                                             // 失敗を確定する（Finish が資源も解放する）。
                                             dispatch_async(g_ctrl, ^{
                                                 @autoreleasepool {
                                                     if (gen == g_generation &&
                                                         g_state == kSoundListening) {
                                                         FandheSoundFinish(kSoundFailed, 9, 0);
                                                         FANDHE_SOUND_NOTIFY();
                                                     }
                                                 }
                                             });
                                         }
                                     }
                                 });
                             }];
            if (![engine startAndReturnError:&error]) {
                FandheSoundTeardown();
                return 9;
            }
            // 開始中のグラフ変更を InputChanged と誤認しないよう、start 成功後に登録する。
            g_config_token = [[NSNotificationCenter defaultCenter]
                addObserverForName:AVAudioEngineConfigurationChangeNotification
                            object:engine
                             queue:nil
                        usingBlock:^(NSNotification *__unused note) {
                            dispatch_async(g_ctrl, ^{
                                @autoreleasepool {
                                    if (gen == g_generation && g_state == kSoundListening) {
                                        FandheSoundFinish(kSoundInterrupted, 0, 3);
                                        FANDHE_SOUND_NOTIFY();
                                    }
                                }
                            });
                        }];
#if TARGET_OS_IPHONE
            AVAudioSession *session = [AVAudioSession sharedInstance];
            void (^interrupt)(void) = ^{
                dispatch_async(g_ctrl, ^{
                    @autoreleasepool {
                        if (gen == g_generation && g_state == kSoundListening) {
                            FandheSoundFinish(kSoundInterrupted, 0, 2);
                            FANDHE_SOUND_NOTIFY();
                        }
                    }
                });
            };
            g_interrupt_token = [[NSNotificationCenter defaultCenter]
                addObserverForName:AVAudioSessionInterruptionNotification
                            object:session
                             queue:nil
                        usingBlock:^(NSNotification *note) {
                            // Began のときだけ中断する。Ended では再開しない。
                            NSNumber *type = note.userInfo[AVAudioSessionInterruptionTypeKey];
                            if (type != nil &&
                                type.unsignedIntegerValue == AVAudioSessionInterruptionTypeBegan) {
                                interrupt();
                            }
                        }];
            // メディアサービスのリセットでエンジンが無効になるため、割り込みとして中断する。
            g_reset_token = [[NSNotificationCenter defaultCenter]
                addObserverForName:AVAudioSessionMediaServicesWereResetNotification
                            object:session
                             queue:nil
                        usingBlock:^(NSNotification *__unused note) {
                            interrupt();
                        }];
#endif
            g_state = kSoundListening;
            g_has_result = NO;
            g_count = 0;
            return 0;
        } @catch (NSException *e) {
            FandheSoundTeardown();
            return 9;
        }
    }
    return 1;
}

int32_t fandhe_apple_sound_classification_start(void) {
    FandheSoundEnsureQueues();
    __block int32_t code = 9;
    dispatch_sync(g_ctrl, ^{
        @autoreleasepool {
            if (g_state == kSoundAwaitingPermission || g_state == kSoundListening) {
                code = 0;
                return;
            }
            // 中断・失敗の残りがあれば初期状態へ戻してから始める。
            FandheSoundFinish(kSoundIdle, 9, 0);
            if (@available(macOS 12.0, iOS 15.0, *)) {
            } else {
                code = 1;
                return;
            }
            g_generation++;
#if TARGET_OS_IPHONE
            if (g_background) {
                // 背景では録音を始めない。成功は偽らず、中断（背景）として read に返す。
                g_state = kSoundInterrupted;
                g_interrupt_reason = 1;
                code = 0;
                return;
            }
#endif
            AVAuthorizationStatus status =
                [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
            if (status == AVAuthorizationStatusAuthorized) {
                int32_t r = FandheSoundBegin();
                if (r != 0) {
                    FandheSoundFinish(kSoundIdle, 9, 0);
                }
                code = r;
            } else if (status == AVAuthorizationStatusNotDetermined) {
                g_state = kSoundAwaitingPermission;
                uint64_t gen = g_generation;
                [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                                         completionHandler:^(BOOL granted) {
                                             // 応答は制御キューへ戻し、世代と状態を確かめてから進める。
                                             dispatch_async(g_ctrl, ^{
                                                 @autoreleasepool {
                                                     if (gen != g_generation ||
                                                         g_state != kSoundAwaitingPermission) {
                                                         return;
                                                     }
                                                     if (!granted) {
                                                         FandheSoundFinish(kSoundFailed, 5, 0);
                                                         FANDHE_SOUND_NOTIFY();
                                                         return;
                                                     }
#if TARGET_OS_IPHONE
                                                     if (g_background) {
                                                         // 応答が背景遷移の後に届いた。録音は始めない。
                                                         FandheSoundFinish(kSoundInterrupted, 0, 1);
                                                         FANDHE_SOUND_NOTIFY();
                                                         return;
                                                     }
#endif
                                                     int32_t r = FandheSoundBegin();
                                                     if (r != 0) {
                                                         FandheSoundFinish(kSoundFailed,
                                                                           r == 3 ? 6 : 9, 0);
                                                     }
                                                     FANDHE_SOUND_NOTIFY();
                                                 }
                                             });
                                         }];
                code = 0;
            } else {
                // denied / restricted
                code = 2;
            }
        }
    });
    return code;
}

int32_t fandhe_apple_sound_classification_read(uint64_t *seq, uint8_t *labels, uint32_t *label_lens,
                                               double *confidences, uint32_t *count, int32_t *reason) {
    FandheSoundEnsureQueues();
    if (seq == NULL || labels == NULL || label_lens == NULL || confidences == NULL || count == NULL ||
        reason == NULL) {
        return 9;
    }
    __block int32_t code = 0;
    dispatch_sync(g_ctrl, ^{
        @autoreleasepool {
            *seq = g_seq;
            *count = 0;
            *reason = 0;
            switch (g_state) {
                case kSoundAwaitingPermission:
                    code = 1;
                    break;
                case kSoundListening:
                    if (g_has_result) {
                        memcpy(labels, g_labels, sizeof(g_labels));
                        memcpy(label_lens, g_label_lens, sizeof(g_label_lens));
                        memcpy(confidences, g_confidences, sizeof(g_confidences));
                        *count = g_count;
                        code = 3;
                    } else {
                        code = 2;
                    }
                    break;
                case kSoundInterrupted:
                    *reason = g_interrupt_reason;
                    code = 4;
                    break;
                case kSoundFailed:
                    code = g_fail_code;
                    break;
                default:
                    code = 0;
                    break;
            }
        }
    });
    return code;
}

void fandhe_apple_sound_classification_stop(void) {
    FandheSoundEnsureQueues();
    dispatch_async(g_ctrl, ^{
        @autoreleasepool {
            g_generation++;
            FandheSoundFinish(kSoundIdle, 9, 0);
        }
    });
}
