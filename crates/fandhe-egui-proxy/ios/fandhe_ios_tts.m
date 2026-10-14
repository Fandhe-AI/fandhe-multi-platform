// Phase 11（親 #864）型サンプル「読み上げ」: iOS 側の実実装（Issue #866）。
//
// `crates/fandhe-egui-proxy/src/device/ios.rs`（`IosTts`）から C ABI 経由で
// 呼ばれる。`AVSpeechSynthesizer` は Rust バインディングを持たないため、この
// 1 ファイルの ObjC コードに実装を閉じ、`extern "C"` 宣言のみを Rust 側へ公開する
// （`fandhe_ios_clipboard.m` と同型、coding-rust.md の FFI 例外規定）。
//
// `build.rs` が iOS ターゲット（`CARGO_CFG_TARGET_OS == "ios"`）かつ feature `device`
// 有効時のみ `cc` でコンパイルする（他ターゲットのビルドには一切関与しない）。
// AVFoundation は `fandhe_ios_camera.m` 側で既にリンク済みのため追加リンクは無い。
//
// 契約（`ios.rs` の `IosTts` doc と対）:
//   fandhe_ios_tts_speak(text, request_generation) -> 0 で成功（発話を開始した）。
//     `text` は NUL 終端の UTF-8 文字列で、本関数はコピーを取るため呼び出し後に
//     Rust 側が解放してよい。`request_generation` は `Tts::speak` が払い出した
//     世代 id をそのまま渡したもの（Rust 側 `IosTts::next_generation` 参照）。
//     戻り値 0 の場合、真の最終結果（発話完了 or 中断）は本関数の戻りでは決まらず、
//     後から [`fandhe_rust_tts_outcome`] を介して非同期に Rust 側へ通知される
//     （codex-review 指摘 PR #869: 旧実装は本関数が 0 を返した時点＝キュー投入成功を
//     `Tts::wait_outcome` が発話完了と混同していた。`AVSpeechSynthesizerDelegate` の
//     `didFinishSpeechUtterance:`/`didCancelSpeechUtterance:` で決着させる）。
//     1 = 端末に音声資産が無い等で発話できない（`[AVSpeechSynthesisVoice
//     speechVoices]` が空）。2 = 内部エラー（NULL 引数・UTF-8 として解釈できない・
//     `AVSpeechSynthesizer` の確保失敗）。1/2 はいずれもコールバックが来ないため、
//     Rust 側は `wait_outcome` へ登録せず即座にエラーを確定させる契約。
//     直前の発話が続いていれば、新しい発話を始める前に必ず中断する
//     （`fandhe_device::tts::Tts::speak` の契約と対称。呼び出し元はワーカースレッド
//     から呼ぶため、直前の発話の完了を待たずに次の発話が来ても壊れない）。
//   fandhe_ios_tts_stop(void) -> 0 で成功。発話していなければ何もせず 0 を返す
//     （`Tts::stop` の契約「発話していなければ何もせず Ok」と対）。停止によって
//     中断された発話があれば `didCancelSpeechUtterance:` が自動的に発火し、
//     その世代の `wait_outcome` を `Ok(())` で解決する（`AVSpeechSynthesizerDelegate`
//     の契約。ユーザー操作による停止を失敗として扱わない、`CommandTts::wait_outcome`
//     doc と同じ判断）。
//
// `fandhe_rust_tts_outcome`（Rust 側 `ios.rs` が公開する `extern "C"` 関数）は
// AVFoundation が提供する delegate コールバックのスレッド（メインスレッド想定だが
// Apple のドキュメントに明記は無いため保証しない）から呼ばれる。Rust 側はどの
// スレッドからの呼び出しにも安全な `Mutex`/`Condvar` で受ける
// （`ios.rs` の `TtsOutcome` 参照）。
//
// 呼び出しスレッド契約: proxy の TTS ワーカースレッド 1 本（`device/tts.rs` の
// `worker_loop`）からのみを想定する。`UIPasteboard`/`CMMotionManager` と異なり
// `AVSpeechSynthesizer` は main スレッド専用の契約を持たない（Apple のドキュメントに
// main スレッド限定の明記が無い）ため、`ios.rs` の `is_main_thread()` 検査を
// 付けない（`ios.rs` の `IosTts` doc に理由を記載）。
//
// 読み上げ内容はログに出力しない（security.md A09。ユーザー入力が機密文字列を
// 含み得るため）。発話の真の完了・中断は `FandheTtsDelegate`（`didFinishSpeechUtterance:`/
// `didCancelSpeechUtterance:`）が拾い、`fandhe_rust_tts_outcome` 経由で Rust 側
// `wait_outcome` へ届ける（下記 `FandheTtsDelegate` 参照、PR #869 是正）。
//
// codex-review P1 指摘の是正: 各関数本体を `@autoreleasepool` で囲む。
// 呼び出し元は proxy の TTS ワーカースレッド 1 本（`device/tts.rs` の
// `worker_loop`）であり、`main` スレッドではないため ARC が pool を自動生成しない
// （Apple のドキュメント: 各スレッドは独自の autorelease pool を管理する必要がある）。
// pool が無いまま `NSString`/`AVSpeechUtterance` 等の一時オブジェクトが
// ワーカースレッドの生存期間中ずっと解放されずに蓄積するのを防ぐ。要求 1 件
// （関数 1 回の呼び出し）を単位に pool を張り、早期 return は pool ブロックの
// 内側からでも問題ない（Objective-C の `@autoreleasepool { return ...; }` は
// pool を正しく閉じてから抜ける）。`dispatch_async` で main queue へ積む遅延発話
// ブロック自体は main の run loop が管理する pool の下で実行されるため、ここでは
// 囲まない（囲む対象はあくまで本関数（ワーカースレッド側）の同期区間）。

#import <AVFoundation/AVFoundation.h>
#import <objc/runtime.h>
#include <os/lock.h>
#include <stdatomic.h>
#include <stdint.h>

// Rust 側（`crates/fandhe-egui-proxy/src/device/ios.rs`）が公開する `extern "C"`
// 関数。世代 `request_generation` の真の最終結果が確定した（発話完了 or 中断）
// ことを通知する。`cancelled` は 0 = 発話完了（成功）、1 = 中断（キャンセル、
// `Tts::wait_outcome` の契約上どちらも `Ok(())` に写像される。AVFoundation の
// `AVSpeechSynthesizerDelegate` は発話失敗を通知する API を持たないため、
// キュー投入後の非同期失敗は存在しない前提（モジュール冒頭コメント参照）。
extern void fandhe_rust_tts_outcome(uint64_t request_generation, int32_t cancelled);

// プロセス内で 1 個だけ保持する synthesizer（`fandhe_ios_motion.m` の
// `g_motion_manager` と同じ dispatch_once パターン）。
static AVSpeechSynthesizer *g_synthesizer = nil;
static dispatch_once_t g_synthesizer_once;

// `AVSpeechUtterance` に紐付ける Rust 側世代 id（`request_generation`）の
// associated object キー（`objc_setAssociatedObject`/`objc_getAssociatedObject`
// はポインタの同一性のみを見るため、値そのものは使わない静的変数）。
static char kFandheTtsGenerationKey;

// `utterance` に紐付けた世代へ結果を通知する（delegate コールバック・遅延発話の
// 破棄経路の双方から呼ばれる共通処理）。
static void FandheReportUtteranceOutcome(AVSpeechUtterance *utterance, int32_t cancelled) {
    NSNumber *generation = objc_getAssociatedObject(utterance, &kFandheTtsGenerationKey);
    if (generation == nil) {
        // 世代が紐付いていない（契約違反を検出した経路。`fandhe_ios_tts_speak` は
        // 必ず紐付けてから発話するため通常到達しない）。
        return;
    }
    fandhe_rust_tts_outcome(generation.unsignedLongLongValue, cancelled);
}

// `AVSpeechSynthesizerDelegate`: 発話の真の完了・中断を Rust 側へ中継する
// （codex-review 指摘 PR #869。旧実装は `fandhe_ios_tts_speak` の戻り値＝
// キュー投入成功を発話完了と混同していた）。
@interface FandheTtsDelegate : NSObject <AVSpeechSynthesizerDelegate>
@end

@implementation FandheTtsDelegate

- (void)speechSynthesizer:(AVSpeechSynthesizer *)synthesizer
    didFinishSpeechUtterance:(AVSpeechUtterance *)utterance {
    FandheReportUtteranceOutcome(utterance, 0);
}

- (void)speechSynthesizer:(AVSpeechSynthesizer *)synthesizer
    didCancelSpeechUtterance:(AVSpeechUtterance *)utterance {
    FandheReportUtteranceOutcome(utterance, 1);
}

@end

// `g_synthesizer.delegate` は弱参照のため、delegate 自体をプロセス内で保持する
// （`g_synthesizer` と同じ dispatch_once ブロックで 1 回だけ生成・設定する）。
static FandheTtsDelegate *g_tts_delegate = nil;

// 遅延発話ブロックの破棄判定に使う内部エポックカウンタ（codex P1 / cursor Bugbot
// 指摘: メインキューへ post した遅延発話が、実行前に stop や後続 speak に
// 追い越されて後から発話してしまう競合の対処）。`speak`/`stop` はいずれも本
// カウンタを進め、メインキューへ積んだ発話ブロックは実行時点のエポックが
// 「積んだ時点のエポック」と一致する場合のみ実発話する。ワーカースレッド
// （呼び出し元）とメインキュー（ブロック実行）の 2 スレッドから読み書きするため
// atomic にする。Rust 側の世代 id（`request_generation`、outcome 通知の識別子）
// とは別の概念（本カウンタは「積んだブロックが最新か」の判定にのみ使う内部値）。
static atomic_int_least32_t g_tts_epoch = 0;

// エポックの検査（読み）から発話開始（`speakUtterance:`）までを、`stop` 側の
// エポック更新＋実停止と同じロックで排他する（codex-review 指摘 P1: atomic だけでは
// カウンタの整合性しか守れず、遅延ブロックの「検査 → 発話」2 手順の間に
// ワーカースレッドの `stop` が割り込む競合を防げない）。`speak`（isSpeaking 分岐・
// メインキューへ積む遅延ブロック）と `stop` の両方が本ロックを取ってから
// カウンタ更新・`AVSpeechSynthesizer` 操作を行うことで、「検査が通った発話は
// 必ずそのまま発話される」「その後に届く stop はロック解放後まで待たされる」
// の 2 択に限定し、検査と発話の間に stop が忍び込む状態を排除する。
static os_unfair_lock g_tts_lock = OS_UNFAIR_LOCK_INIT;

static AVSpeechSynthesizer *FandheGetSynthesizer(void) {
    dispatch_once(&g_synthesizer_once, ^{
        g_synthesizer = [[AVSpeechSynthesizer alloc] init];
        g_tts_delegate = [[FandheTtsDelegate alloc] init];
        g_synthesizer.delegate = g_tts_delegate;
    });
    return g_synthesizer;
}

int32_t fandhe_ios_tts_speak(const char *text, uint64_t request_generation) {
    @autoreleasepool {
        if (text == NULL) {
            return 2;
        }
        NSString *value = [NSString stringWithUTF8String:text];
        if (value == nil) {
            // UTF-8 として解釈できないバイト列（Rust 側の `CString` 経由では起きないが、
            // 契約違反を検出した経路として安全側で返す）。
            return 2;
        }

        if (AVSpeechSynthesisVoice.speechVoices.count == 0) {
            // 端末に音声資産が無い（シミュレータの一部構成等）。機種として非対応。
            return 1;
        }

        AVSpeechSynthesizer *synthesizer = FandheGetSynthesizer();
        if (synthesizer == nil) {
            return 2;
        }

        AVSpeechUtterance *utterance = [AVSpeechUtterance speechUtteranceWithString:value];
        if (utterance == nil) {
            return 2;
        }
        // 発話完了・中断の通知（delegate コールバック）をこの世代 id へ結び付ける
        // （`FandheReportUtteranceOutcome` 参照。以後この utterance に対して来る
        // finish/cancel はすべてこの世代として Rust 側へ返す）。
        objc_setAssociatedObject(utterance, &kFandheTtsGenerationKey,
                                  @(request_generation), OBJC_ASSOCIATION_RETAIN);
        // 音声（言語）は指定せず OS 既定に任せる（端末言語に従う。#866 実装計画 §3-8、
        // 言語自動判定は天井として明示: ponytail: 現状は端末既定言語の音声のみ、
        // 発話テキストの言語判定による切り替えが要るなら別途対応する）。

        // 直前の発話が続いていれば中断してから始める（`Tts::speak` の契約）。
        // `stopSpeakingAtBoundary:` は BOOL を返して即座に戻るが、内部の後片付け
        // （オーディオセッションの解放等）は戻り値確定後も続くことがあり、直後に
        // 同期で `speakUtterance:` を呼ぶと新しい発話がその後片付けに巻き込まれて
        // 無音のまま破棄されることがある（cursor Bugbot 指摘）。次の実行ループへ
        // 回してから発話することで後片付けの完了を待つ（同種の問題への既知の回避策）。
        //
        // ただしメインキューへ積んだ時点から実行されるまでの間に、ワーカースレッドが
        // 次の `stop`/`speak` を処理してしまうと、停止・後続発話を報告済みなのに
        // 積んでおいたこの発話が後から始まってしまう（codex P1 / cursor Bugbot 指摘。
        // proxy の FIFO による発話・停止順序保証が崩れる）。世代カウンタで積んだ
        // 時点の世代を閉じ込め、実行時に最新世代と一致する場合のみ発話する。
        // 検査（`atomic_load` の一致確認）から発話開始（`speakUtterance:`）までを
        // `g_tts_lock` で保護し、`fandhe_ios_tts_stop` 側の世代更新＋実停止と排他する
        // （codex P1 再指摘: メインキューはブロック内の実行順序こそ保証するが、
        // ワーカースレッドで直接動く `stop` 側の処理とは別スレッドのため、検査直後・
        // 発話直前の隙間に `stop` が割り込む競合は世代カウンタの atomic 性だけでは
        // 防げない）。
        // `isSpeaking` の判定自体も `g_tts_lock` の下で行う（codex-review 指摘 P1）。
        // ロック外で判定すると、A の発話を B が停止して遅延ブロックを積んだ直後、
        // C がロック未取得のまま `isSpeaking == false`（B の停止が反映済み）を
        // 観測し、C は「停止すべき発話が無い」と誤認して else 分岐（stop を挟まず
        // 即 `speakUtterance:`）へ進んでしまう。`AVSpeechSynthesizer` はキュー型
        // のため、この即時発話が先に鳴り、後から実行される B の遅延発話が後に
        // 鳴ってしまい、要求順（B → C）と発話順が入れ替わる。判定から世代更新・
        // 停止（または即時発話）までを 1 ロック区間に閉じることで、この区間に
        // 他スレッドの `speak`/`stop` が割り込めないようにする。
        os_unfair_lock_lock(&g_tts_lock);
        if (synthesizer.isSpeaking) {
            int32_t epoch = atomic_fetch_add(&g_tts_epoch, 1) + 1;
            [synthesizer stopSpeakingAtBoundary:AVSpeechBoundaryImmediate];
            os_unfair_lock_unlock(&g_tts_lock);
            dispatch_async(dispatch_get_main_queue(), ^{
                os_unfair_lock_lock(&g_tts_lock);
                if (atomic_load(&g_tts_epoch) != epoch) {
                    // 積んだ後に stop または新しい speak が来てエポックが進んだ。
                    // この発話はもう「最新の要求」ではないため破棄する。破棄した
                    // utterance は一度も speakUtterance: されないため
                    // AVSpeechSynthesizerDelegate の finish/cancel は永遠に来ない。
                    // Rust 側 wait_outcome が待ち続けないよう、ここでキャンセル
                    // 相当として明示的に決着させる（codex-review 指摘 PR #869）。
                    os_unfair_lock_unlock(&g_tts_lock);
                    FandheReportUtteranceOutcome(utterance, 1);
                    return;
                }
                [synthesizer speakUtterance:utterance];
                os_unfair_lock_unlock(&g_tts_lock);
            });
        } else {
            atomic_fetch_add(&g_tts_epoch, 1);
            [synthesizer speakUtterance:utterance];
            os_unfair_lock_unlock(&g_tts_lock);
        }
        return 0;
    }
}

int32_t fandhe_ios_tts_stop(void) {
    @autoreleasepool {
        // メインキューへ積まれている可能性のある遅延発話ブロックを無効化する
        // （エポックを進める。上記 `fandhe_ios_tts_speak` 参照）。エポック更新と
        // 実停止を `g_tts_lock` の下で行うことで、遅延ブロックの「検査 → 発話」
        // 区間との排他を成立させる（`g_tts_lock` 宣言部のコメント参照）。
        os_unfair_lock_lock(&g_tts_lock);
        atomic_fetch_add(&g_tts_epoch, 1);
        if (g_synthesizer == nil) {
            // まだ 1 度も発話していない（synthesizer 未生成）。発話していないのと
            // 同じ状態のため成功として扱う（`Tts::stop` の契約）。
            os_unfair_lock_unlock(&g_tts_lock);
            return 0;
        }
        [g_synthesizer stopSpeakingAtBoundary:AVSpeechBoundaryImmediate];
        os_unfair_lock_unlock(&g_tts_lock);
        return 0;
    }
}
