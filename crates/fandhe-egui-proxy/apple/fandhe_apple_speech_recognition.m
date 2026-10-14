// Phase 11 の拡充ループ（親 #1023）型サンプル「音声認識」: macOS 実装（Issue #1024）+
// iOS 実装（Issue #1025）。iOS / macOS 共用ファイル（`fandhe_apple_mic_record.m`/
// `fandhe_apple_location.m` と同じ判断）。Android/Web は兄弟 issue が担当する。
// iOS 側は `#if TARGET_OS_IPHONE` で以下を追加する:
//   - 聞き取り直前に `AVAudioSession` の category を Record にして `setActive:YES`
//     （マイク使用を OS へ宣言する。macOS の素の実行体には `AVAudioSession` の
//     概念が無いため macOS では no-op、下記「AVAudioSession」節参照）
//   - 完了時（成功・失敗いずれも）に有効化前の category / options へ復元する
//   - バックグラウンド遷移（`UIApplicationDidEnterBackgroundNotification`）で
//     破棄して新設 code=5 で完了通知する（バックグラウンド聞き取りは v1 の範囲外、
//     下記「バックグラウンド」節参照）
// `build.rs` は macOS・iOS 双方の分岐からコンパイルする。
//
// `crates/fandhe-egui-proxy/src/device/apple_speech_recognition.rs` から C ABI 経由で
// 呼ばれる。
//
// 契約（pull 型。`fandhe_apple_mic_record.m` と同型、coding-rust.md の FFI 例外規定）:
//   fandhe_apple_speech_start(uint64_t request_id, double max_seconds)
//     -> int32_t 戻り値: 常に 0（依頼を受理し dispatch_async(main) へ積んで
//        即座に戻る）。二重開始の判定自体も main queue 上でのみ行う
//        （`g_speech_context` への全アクセスを main queue に閉じる方針、下記
//        「スレッド」節参照）ため、呼び出し元スレッドで同期的に判定しない。
//        結果（成功・失敗・busy のいずれも）は非同期に
//        fandhe_rust_speech_finished で 1 回だけ届く。
//   fandhe_apple_speech_stop(uint64_t request_id)
//     -> int32_t 戻り値: 常に 0（依頼を dispatch_async(main) へ積んで即座に
//        戻る、`fandhe_apple_speech_start` と同じ非同期方針）。main queue 上で
//        request_id が現在進行中のセッションと一致しない場合は無視され、
//        コールバックは呼ばれない（`fandhe_apple_mic_record_stop` と同じ
//        非同期の取り違え防止、下記実装参照）。
//   fandhe_rust_speech_finished(uint64_t request_id, int32_t code, const char *text)
//     -> Rust 側（`apple_speech_recognition.rs`）が公開する `extern "C"` 関数。
//        code: 0 = 成功（`text` が有効、空文字列もあり得る）。1 = 権限拒否。
//        2 = 内部エラー（エンジン起動失敗等）。3 = 二重開始（busy）。
//        4 = 許可確認中に「停止」でキャンセルされた。6 = 認識器を作れない
//        （`SFSpeechRecognizer` が nil・言語未対応）。7 = 認識器が一時的に
//        使えない（`isAvailable == NO`）。8 = 無音（結果 0 件）。5 = バックグラウンド
//        遷移により破棄された（iOS のみ、下記「バックグラウンド」節参照）。
//        0 以外では text を渡さない（NULL）。text が指す領域はこの呼び出しが
//        戻るまでのみ有効（ローカル変数）。Rust 側はこの呼び出し内で同期的に
//        コピーし切る契約（`.rs` 側コメント参照）。
//   fandhe_rust_speech_started(uint64_t request_id)
//     -> Rust 側が公開する `extern "C"` 関数。`recognitionTaskWithRequest:` が
//        実際に開始した瞬間（権限確認完了後）に 1 回だけ呼ぶ。権限確認待ちの
//        間は呼ばない（Rust 側はこれを見て経過時間の計測を始める、
//        device-bridge-contracts.md「期限・キャンセル」節）。
//
// 権限: `SFSpeechRecognizer.authorizationStatus` で既知の状態
// （authorized/denied/restricted）を同期判定し、denied/restricted は即座に
// code=1 で完了通知する。notDetermined のときのみ
// `requestAuthorization:completionHandler:` で非同期に確認する。加えて
// `AVCaptureDevice.authorizationStatusForMediaType:AVMediaTypeAudio` でマイク
// 権限も明示確認する（拒否時に無音の入力が届いて「聞き取れず」へ化ける嘘を
// 防ぐ、実装計画 §3.4 手順 3）。
//
// 聞き取り: 許可が得られたら `SFSpeechRecognizer` と
// `SFSpeechAudioBufferRecognitionRequest`（`shouldReportPartialResults = YES`）を
// 構築し、`AVAudioEngine.inputNode` から `installTapOnBus:` でバッファを流し込む。
// `dispatch_after(max_seconds)` で `endAudio` を呼び、OS 側の期限を Rust 側
// （`MAX_LISTEN_MS`）と揃える。
//
// 停止・完了: 「停止」は `endAudio` を呼んで結果の確定を OS に求めるだけで、
// `recognitionTaskWithRequest:` の `resultHandler`（`isFinal == YES` の呼び出し）
// でのみ完了通知する。`endAudio` 後、猶予（`kFandheSpeechFinishGraceSeconds`）内に
// 確定しなければ最後の途中結果（無ければ無音）で確定する（実装計画 §2.3）。
//
// 二重開始防御: `g_speech_context != nil` の間に `fandhe_apple_speech_start` が
// 呼ばれた場合、進行中のセッションを潰さず、新しい request_id へ code=3（busy）で
// 即座に完了通知する（`fandhe_apple_mic_record.m` の同種の分岐と同じ位置づけ）。
//
// スレッド: 開始・停止依頼・`resultHandler`/`requestAuthorization` の completion は
// いずれも main queue に閉じる（`g_speech_context` への全アクセスを main queue に
// 閉じ、形式的なデータレースも作らない、`fandhe_apple_mic_record.m` と同じ方針）。
//
// 許可確認中のキャンセル: `g_speech_context` は「聞き取り中」だけでなく
// 「許可確認中」も含めて 1 つの世代として `FandheSpeechStartOnMainThread` の冒頭で
// 予約する。`fandhe_apple_speech_stop` は `g_speech_task`（実際の認識タスク）が
// まだ nil ならこの予約段階と判断し、`cancelled` を立てて即座に code=4 で完了
// 通知する。`requestAuthorization:`/マイク権限確認の completion は許可が下りても
// `cancelled` を確認してから聞き取りを開始する。
//
// 後始末（`FandheSpeechFinish`）: 1 箇所に集約し、成功・失敗・キャンセル・
// 開始途中失敗のどの経路でも同じ処理（タスク取り消し・エンジン停止・タップ除去・
// `endAudio`・グローバル解放）を通す（実装計画 §3.4「後始末」節、
// device-bridge-contracts.md「失敗時のリソース解放」節）。
//
// 情報非保持: 認識文字列・途中結果・音声バッファは `NSLog` を含むログへ一切
// 出力しない（security.md A09）。
//
// デスクトップは 1 画面 = 1 プロセスのため、画面離脱（Activity 再生成・bfcache
// 復帰相当）は扱わない（プロセス終了のみ、実装計画 §2.2「画面離脱」節）。
//
// AVAudioSession（iOS のみ、Issue #1025）: 聞き取り直前に category を
// `AVAudioSessionCategoryRecord` にして `setActive:YES` する。macOS の素の実行体には
// `AVAudioSession` の概念が無いため、macOS では no-op マクロにしている
// （TARGET_OS_IPHONE で分岐）。有効化前の category / categoryOptions はセッション
// ごとの `FandheSpeechContext` のプロパティに保持し（`fandhe_apple_mic_record.m` の
// static 変数と異なり、後始末を `FandheSpeechFinish`/`FandheSpeechFinalize` の
// 1 か所に既に集約済みのため、context 単位で持てば「自分が有効化したときだけ
// 解放する」が自然に書ける）、完了時（成功・失敗いずれも、`FandheSpeechFinalize`
// 内）に `setActive:NO` して解放したうえで両方を復元する（他アプリへの通知が目的で
// 失敗は無視する。`fandhe_apple_mic_record.m` の同種の判断と同じ）。
//
// 本ファイルが `AVAudioSession` を直接扱うのは iOS 分岐のみ。共有セッションの
// 一元管理（誰が有効化したかを横断的に記録し、自分が有効化した場合のみ解放する）は
// 持たない: `FANDHE_PROXY_APP` は 1 プロセス 1 画面のため、本モジュールと
// `fandhe_apple_mic_record.m` が同一プロセスで同時に動く経路は無い
// （`fandhe_apple_mic_record.m` 冒頭コメント「共有セッションを触る 2 つ目の機能を
// 足す時点で一元管理が必要になる」の「2 つ目の機能」に本モジュールが該当するが、
// 同時実行が無いため衝突しない）。同一プロセスで 2 画面を同時に出す日が来たら
// 一元管理へ移行する。
//
// バックグラウンド（iOS のみ、Issue #1025）: `UIApplicationDidEnterBackgroundNotification`
// を購読し（`fandhe_apple_mic_record.m`/`fandhe_apple_gamepad.m` の `dispatch_once`
// 購読と同じ書式）、進行中セッション（聞き取り中・許可確認中のいずれも）があれば
// 直ちに破棄扱い（`interrupted`/`cancelled` を立てて `FandheSpeechFinish(context, 5,
// nil)`）にする。`FandheSpeechFinish` は聞き取り未開始（`task`/`engine` が nil）でも
// 安全に呼べる（`FandheSpeechStopAudioInput`/`FandheSpeechDeactivateSession` は nil
// 安全）ため、聞き取り中・許可確認中を分岐させず 1 本で済ませられる
// （`fandhe_apple_mic_record.m` は録音実体の有無で分岐が必要だったが、本モジュールは
// 既に後始末が 1 箇所に集約済みのため分岐が不要）。
//
// `resultHandler`/期限到達のコールバックがバックグラウンド遷移の通知より後に main
// queue へ届く競合（`fandhe_apple_mic_record.m` の Bugbot #961 Medium 指摘と同じ
// 種類）への対処として、`FandheSpeechFinish` は成功・失敗を問わず到達時に現在の
// `UIApplicationState` を都度確認する（PR #1029 の codex/Bugbot 指摘により、
// 当初の「成功コード（0）到達時のみ確認」から拡張した。バックグラウンド遷移は
// `resultHandler` へエラー・無音を先に届けることがあり、code==0 に限定すると
// バックグラウンド起因の失敗が internal failure/NoMatch へ化けるため）:
// Background なら即座に code=5 へ上書きし、Inactive（Control Center 等、
// フォアグラウンドのままの一時的な状態）ならマイク入力を即座に止めたうえで
// `kFandheSpeechInactiveGraceSeconds` だけ確定を猶予し、猶予後に本物のバックグラウンド
// 遷移が起きていなければ元の code のまま確定する（`fandhe_apple_mic_record.m` の
// `finishWithCode:`/`finalizeWithCode:` と同じ二段構成、実処理は
// [`FandheSpeechFinalize`] に切り出す）。

#import <TargetConditionals.h>
#import <AVFoundation/AVFoundation.h>
#import <Speech/Speech.h>
#if TARGET_OS_IPHONE
#import <UIKit/UIKit.h>
#endif

// `endAudio` 後、結果確定を待つ猶予秒数（実装計画 §2.3）。猶予内に確定しなければ
// 最後の途中結果（無ければ無音）で確定する。
static const NSTimeInterval kFandheSpeechFinishGraceSeconds = 3.0;

#if TARGET_OS_IPHONE
// `FandheSpeechFinish` が Inactive を検出したときの猶予秒数（doc 冒頭
// 「バックグラウンド」節参照）。Control Center・通知センター・システム警告等
// フォアグラウンドのままの一時的な Inactive は通常 1 秒未満で Active に戻るため、
// 実際にバックグラウンドへ遷移する場合との判別に十分な長さとして
// `fandhe_apple_mic_record.m` と同じ 0.5 秒とした（実測に基づく調整可能な値、
// 固定契約ではない）。
static const NSTimeInterval kFandheSpeechInactiveGraceSeconds = 0.5;
#endif

// Rust 側（`apple_speech_recognition.rs`）が公開する `extern "C"` 関数。
extern void fandhe_rust_speech_finished(uint64_t request_id, int32_t code, const char *text);

// `recognitionTaskWithRequest:` が実際に開始した瞬間に呼ぶ（Rust 側が権限確認中の
// 待ち時間を聞き取り時間として数え始めないための合図）。
extern void fandhe_rust_speech_started(uint64_t request_id);

// 進行中セッションの文脈（`FandheSpeechFinish` が reply を待つ間・後始末を行う間、
// プロセス内で生存させる必要があるため static で強参照する、下記
// `g_speech_context` 参照）。
@interface FandheSpeechContext : NSObject
@property(nonatomic, assign) uint64_t requestId;
// 許可確認中に「停止」が押されたことを示す（doc 冒頭「許可確認中のキャンセル」
// 節参照）。true のときは許可が下りても聞き取りを開始しない。
@property(nonatomic, assign) BOOL cancelled;
// `FandheSpeechFinish` の二重実行防御（`resultHandler` の `isFinal` 到着と
// 猶予タイマの両方から呼ばれ得るため、1 回目で立てて 2 回目以降は即座に
// return する）。
@property(nonatomic, assign) BOOL finished;
// 直近の途中結果（`shouldReportPartialResults = YES` で届く）。猶予切れで
// 確定する際にこれを使う（無ければ無音、実装計画 §2.3）。ログにも画面にも
// 出さない（メモリ保持のみ、doc 冒頭「聞き取り」節参照）。
@property(nonatomic, copy) NSString *lastPartialTranscript;
@property(nonatomic, strong) SFSpeechRecognizer *recognizer;
@property(nonatomic, strong) SFSpeechAudioBufferRecognitionRequest *request;
@property(nonatomic, strong) SFSpeechRecognitionTask *task;
@property(nonatomic, strong) AVAudioEngine *engine;
// バックグラウンド遷移（iOS のみ）で破棄されたことを示す（doc 冒頭
// 「バックグラウンド」節参照）。true のときは `FandheSpeechFinish` が成功コードを
// 5 へ上書きする。
@property(nonatomic, assign) BOOL interrupted;
// iOS のみ（doc 冒頭「AVAudioSession」節参照）。`FandheSpeechActivateSession` が
// 成功したときだけ YES にし、`FandheSpeechDeactivateSession` が二重復元を防ぐために
// 参照する。
@property(nonatomic, assign) BOOL sessionActivated;
@property(nonatomic, copy) NSString *previousCategory;
@property(nonatomic, assign) AVAudioSessionCategoryOptions previousCategoryOptions;
@end

@implementation FandheSpeechContext
@end

// 進行中セッション。`g_speech_context` が nil のときのみ新規セッションを開始
// できる（二重開始防御、上記コメント参照）。全アクセスを main queue に閉じる
// （上記「スレッド」節参照）。
static FandheSpeechContext *g_speech_context = nil;

// エンジン停止・タップ除去・`endAudio` をまとめる（`FandheSpeechFinish` と
// 「停止」要求・期限到達の両方から呼ばれる冪等な下請け）。`engine.isRunning` で
// 二重停止を防ぐ。マイク入力の遮断は結果確定を待たず即座に行う必要がある
// （codex 指摘: `endAudio` 後もタップが `appendAudioPCMBuffer:` を呼び続け、
// 最終結果または猶予切れまでマイク入力が投入され続けていた）。
static void FandheSpeechStopAudioInput(FandheSpeechContext *context) {
    if (context.engine.isRunning) {
        [context.engine stop];
        [context.engine.inputNode removeTapOnBus:0];
    }
    [context.request endAudio];
}

#if TARGET_OS_IPHONE
// AVAudioSession を Record カテゴリで有効化する（doc 冒頭「AVAudioSession」節参照）。
// 失敗（他アプリとの競合等）は NO を返し、呼び出し元が code=2 で完了通知する。
// 有効化前の category / categoryOptions を `context` に記憶し、解放時に復元する
// （`fandhe_apple_mic_record.m::FandheMicRecordActivateSession` と同じ手順だが、
// 保存先は static ではなく `context` 自身、doc 冒頭参照）。
static BOOL FandheSpeechActivateSession(FandheSpeechContext *context) {
    NSError *error = nil;
    AVAudioSession *session = [AVAudioSession sharedInstance];
    NSString *previousCategory = session.category;
    AVAudioSessionCategoryOptions previousCategoryOptions = session.categoryOptions;
    if (![session setCategory:AVAudioSessionCategoryRecord error:&error]) {
        return NO;
    }
    if (![session setActive:YES error:&error]) {
        // category の変更だけ先に成功しているため、有効化に失敗した場合は元の
        // category / categoryOptions へ戻してから NO を返す（`fandhe_apple_mic_record.m`
        // の同種の分岐と同じ理由: Record のまま共有セッションに残すと、後続の
        // 再生（`AVSpeechSynthesizer` 等）が無音化する）。
        [session setCategory:previousCategory withOptions:previousCategoryOptions error:nil];
        return NO;
    }
    context.previousCategory = previousCategory;
    context.previousCategoryOptions = previousCategoryOptions;
    context.sessionActivated = YES;
    return YES;
}

// セッションを解放し、有効化前の category / categoryOptions へ戻す（doc 冒頭
// 「AVAudioSession」節参照）。`sessionActivated` が立っていない（有効化していない・
// 既に復元済み）ときは no-op にする（二重復元防止）。聞き取りは既に終わっており
// 他アプリへ通知するのが目的のため、失敗は無視する。
static void FandheSpeechDeactivateSession(FandheSpeechContext *context) {
    if (!context.sessionActivated) {
        return;
    }
    AVAudioSession *session = [AVAudioSession sharedInstance];
    [session setActive:NO
           withOptions:AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation
                 error:nil];
    if (context.previousCategory != nil) {
        [session setCategory:context.previousCategory
                  withOptions:context.previousCategoryOptions
                        error:nil];
    }
    context.sessionActivated = NO;
}
#else
// macOS の素の実行体には `AVAudioSession` の概念が無いため no-op にする
// （doc 冒頭「AVAudioSession」節参照）。
static BOOL FandheSpeechActivateSession(__unused FandheSpeechContext *context) {
    return YES;
}
static void FandheSpeechDeactivateSession(__unused FandheSpeechContext *context) {
}
#endif

// `FandheSpeechFinish` から即座に、または Inactive 猶予（doc 冒頭「バックグラウンド」
// 節参照）を経てから 1 回だけ呼ばれる。完了通知・タスク取り消し・エンジン停止・
// タップ除去・`endAudio`・セッション解放・グローバル解放の実処理（doc 冒頭
// 「後始末」節参照）。`context` が現在のセッション（`g_speech_context` と同一）の
// ときだけグローバルを解放する（次のセッションが既に上書きしている可能性への
// 安全網、`fandhe_apple_mic_record.m` の `isCurrentSession` 判定と同じ考え方）。
static void FandheSpeechFinalize(FandheSpeechContext *context, int32_t code, NSString *text) {
    [context.task cancel];
    FandheSpeechStopAudioInput(context);
    FandheSpeechDeactivateSession(context);

    uint64_t requestId = context.requestId;
    if (code == 0) {
        const char *cText = [text UTF8String];
        fandhe_rust_speech_finished(requestId, 0, cText != NULL ? cText : "");
    } else {
        fandhe_rust_speech_finished(requestId, code, NULL);
    }

    if (g_speech_context == context) {
        g_speech_context = nil;
    }
}

// 完了通知の入口を 1 箇所に集約する（doc 冒頭「後始末」節参照）。二重実行防御
// （`finished` プロパティの doc 参照）をここで行い、以降は
// [`FandheSpeechFinalize`] へ委譲する。iOS のみ、バックグラウンド遷移・Inactive
// 猶予の判定をここで行う（doc 冒頭「バックグラウンド」節参照、
// `fandhe_apple_mic_record.m::finishWithCode:`/`finalizeWithCode:` と同じ二段構成）。
static void FandheSpeechFinish(FandheSpeechContext *context, int32_t code, NSString *text) {
    if (context.finished) {
        return;
    }
    context.finished = YES;

    if (context.interrupted) {
        code = 5;
    }
#if TARGET_OS_IPHONE
    // Background/Inactive の判定は成功（code==0）に限定しない。バックグラウンド
    // 遷移は `AVAudioSession` を横取りし、`resultHandler` へ内部エラー・無音等を
    // 先に届けることがあるため、code==0 だけを見ると「バックグラウンド起因の
    // 失敗」が通常の internal failure/NoMatch へ化けてしまう
    // （Cursor Bugbot 指摘、PR #1029）。
    // Background は即座に破棄。Inactive は `FandheSpeechFinalize` の呼び出しを
    // `kFandheSpeechInactiveGraceSeconds` だけ猶予し、猶予後に `context.interrupted`
    // か `applicationState == Background` のいずれかが真なら破棄コード（5）、
    // そうでなければ元の code で確定する（`fandhe_apple_mic_record.m::finishWithCode:`
    // と同じ二段構成）。
    UIApplicationState state = [UIApplication sharedApplication].applicationState;
    if (state == UIApplicationStateBackground) {
        code = 5;
    } else if (state == UIApplicationStateInactive) {
        // マイク入力は猶予を待たず即座に止める（codex 指摘、PR #1029）。猶予中に
        // 本物のバックグラウンド遷移が起きても、通知オブザーバーは `finished` が
        // 既に真であるため何もしない（先頭の二重実行防御）ので、遮断は必ずここで
        // 行っておく必要がある（device-bridge-contracts.md「切断・破棄後に最後の
        // 値を返し続けていないか」節）。
        FandheSpeechStopAudioInput(context);
        int32_t pendingCode = code;
        dispatch_after(
            dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kFandheSpeechInactiveGraceSeconds * NSEC_PER_SEC)),
            dispatch_get_main_queue(), ^{
              int32_t finalCode = pendingCode;
              if (context.interrupted
                  || [UIApplication sharedApplication].applicationState == UIApplicationStateBackground) {
                  finalCode = 5;
              }
              FandheSpeechFinalize(context, finalCode, text);
            });
        return;
    }
#endif

    FandheSpeechFinalize(context, code, text);
}

// 許可確認済み（マイク・音声認識とも authorized）の状態から実際に聞き取りを
// 開始する（doc 冒頭「聞き取り」節参照）。
static void FandheSpeechBeginListening(FandheSpeechContext *context, double maxSeconds) {
    if (context.cancelled) {
        // 許可確認中に「停止」が押された（`fandhe_apple_speech_stop` が既に
        // code=4 で完了通知済み）。二重通知を避けるため何もしない。
        return;
    }

#if TARGET_OS_IPHONE
    // システムの許可ダイアログは通常 DidEnterBackground を発火しないため稀な経路
    // だが、許可確認完了からここに到達するまでの間にバックグラウンドへ遷移した
    // 場合に何もしないと、バックグラウンドのまま聞き取りを開始してしまう
    // （v1 の「バックグラウンド聞き取りは範囲外」契約に反する、doc 冒頭
    // 「バックグラウンド」節参照。`fandhe_apple_mic_record.m` の同種の対応と同じ）。
    if ([UIApplication sharedApplication].applicationState == UIApplicationStateBackground) {
        FandheSpeechFinish(context, 5, nil);
        return;
    }
#endif

    SFSpeechRecognizer *recognizer = [[SFSpeechRecognizer alloc] init];
    if (recognizer == nil) {
        FandheSpeechFinish(context, 6, nil);
        return;
    }
    if (!recognizer.isAvailable) {
        FandheSpeechFinish(context, 7, nil);
        return;
    }
    context.recognizer = recognizer;

    // iOS のみ、マイク使用を OS へ宣言する（doc 冒頭「AVAudioSession」節参照）。
    // macOS は no-op。`AVAudioEngine` の `inputNode` は session 有効化後でないと
    // format が 0Hz になり得るため、エンジン生成前に呼ぶ。
    if (!FandheSpeechActivateSession(context)) {
        FandheSpeechFinish(context, 2, nil);
        return;
    }

    SFSpeechAudioBufferRecognitionRequest *request = [[SFSpeechAudioBufferRecognitionRequest alloc] init];
    request.shouldReportPartialResults = YES;
    context.request = request;

    AVAudioEngine *engine = [[AVAudioEngine alloc] init];
    context.engine = engine;
    AVAudioInputNode *inputNode = engine.inputNode;
    AVAudioFormat *format = [inputNode outputFormatForBus:0];
    if (format.sampleRate == 0) {
        FandheSpeechFinish(context, 2, nil);
        return;
    }

    [inputNode installTapOnBus:0
                     bufferSize:1024
                         format:format
                          block:^(AVAudioPCMBuffer *buffer, __unused AVAudioTime *when) {
                            [request appendAudioPCMBuffer:buffer];
                          }];

    [engine prepare];
    NSError *startError = nil;
    if (![engine startAndReturnError:&startError]) {
        [inputNode removeTapOnBus:0];
        FandheSpeechFinish(context, 2, nil);
        return;
    }

    __weak FandheSpeechContext *weakContext = context;
    SFSpeechRecognitionTask *task = [recognizer
        recognitionTaskWithRequest:request
                     resultHandler:^(SFSpeechRecognitionResult *result, NSError *error) {
                       dispatch_async(dispatch_get_main_queue(), ^{
                         FandheSpeechContext *strongContext = weakContext;
                         if (strongContext == nil || strongContext != g_speech_context) {
                             // 古い世代（キャンセル済み・差し替え済み）のコールバックは
                             // 照合して捨てる（device-bridge-contracts.md「並行性・
                             // 世代」節）。
                             return;
                         }
                         // 確定済みの `result`（`isFinal`）を `error` より先に判定する。
                         // stop 起因の `endAudio` や締切到達では、確定済みの結果と
                         // ハウスキーピング用の `error` が同一コールバックで同時に届く
                         // ことがある。ここで `error` を先に見て `return` すると、
                         // 認識済みの発話を内部失敗として即座に破棄してしまう
                         // （Cursor Bugbot 指摘、PR #1028）。未確定（非 final）の
                         // `result` しかない状態での `error` は、これまでどおり
                         // 内部失敗として扱う（`result` を先に見て何でも成功に畳むと
                         // 嘘の成功に戻るため。device-bridge-contracts.md「嘘の成功」
                         // 節）。
                         if (result != nil) {
                           strongContext.lastPartialTranscript = result.bestTranscription.formattedString;
                           if (result.isFinal) {
                             FandheSpeechFinish(strongContext, 0, strongContext.lastPartialTranscript);
                             return;
                           }
                         }
                         if (error != nil) {
                           // `kAFAssistantErrorDomain` code 1110 は無音（結果 0 件）を
                           // 意味する既知のドメイン・コードで、成功扱いに畳まず専用の
                           // NoMatch へ写す（実装計画 §3.4「完了」節）。それ以外の
                           // エラーは内部エラーへ倒し、成功扱いにしない
                           // （device-bridge-contracts.md「嘘の成功」節）。
                           if ([error.domain isEqualToString:@"kAFAssistantErrorDomain"]
                               && error.code == 1110) {
                             FandheSpeechFinish(strongContext, 8, nil);
                           } else {
                             FandheSpeechFinish(strongContext, 2, nil);
                           }
                           return;
                         }
                       });
                     }];
    context.task = task;

    fandhe_rust_speech_started(context.requestId);

    // OS 側の期限（実装計画 §2.3・§3.4「期限」節）。Rust 側の `MAX_LISTEN_MS`
    // （`maxSeconds` として渡ってくる）と揃える。
    __weak FandheSpeechContext *weakContextForDeadline = context;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(maxSeconds * NSEC_PER_SEC)),
                   dispatch_get_main_queue(), ^{
                     FandheSpeechContext *strongContext = weakContextForDeadline;
                     if (strongContext == nil || strongContext != g_speech_context) {
                       return;
                     }
                     // マイク入力を即座に遮断する（`endAudio` だけではタップが
                     // 動き続け、猶予切れまで音声が投入され続けるため）。
                     FandheSpeechStopAudioInput(strongContext);
                     // 猶予内に `resultHandler` の `isFinal` が届かなければ、最後の
                     // 途中結果（無ければ無音）で確定する（実装計画 §2.3）。
                     dispatch_after(
                         dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kFandheSpeechFinishGraceSeconds * NSEC_PER_SEC)),
                         dispatch_get_main_queue(), ^{
                           FandheSpeechContext *graceContext = weakContextForDeadline;
                           if (graceContext == nil || graceContext != g_speech_context) {
                             return;
                           }
                           NSString *fallback = graceContext.lastPartialTranscript;
                           FandheSpeechFinish(graceContext, 0, fallback != nil ? fallback : @"");
                         });
                   });
}

#if TARGET_OS_IPHONE
// バックグラウンド遷移の購読を 1 回だけ行う（doc 冒頭「バックグラウンド」節・
// `fandhe_apple_mic_record.m::FandheMicRecordEnsureBackgroundObserver` と同じ書式）。
// ハンドラは main queue 上で呼ばれる（`addObserverForName:` の `queue:` 引数）ため、
// `g_speech_context` への全アクセスを main queue に閉じる方針（doc 冒頭「スレッド」
// 節）を保てる。戻り値（observer token）は呼び出し元が強参照し続けない限り ARC が
// 即座に解放し、その dealloc で購読自体が解除される
// （`fandhe_apple_mic_record.m` の同種のコメント参照）ため static で永続的に保持する。
static id g_speech_background_observer = nil;

static void FandheSpeechEnsureBackgroundObserver(void) {
    static dispatch_once_t once_token;
    dispatch_once(&once_token, ^{
        g_speech_background_observer = [[NSNotificationCenter defaultCenter]
            addObserverForName:UIApplicationDidEnterBackgroundNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *_Nonnull __unused note) {
                      if (g_speech_context == nil) {
                          // 進行中セッションなし。何もしていない。
                          return;
                      }
                      // 聞き取り中・許可確認中のどちらもこの 1 本で済む
                      // （`FandheSpeechFinish`/`FandheSpeechFinalize` が既に
                      // 後始末を 1 箇所に集約済みで、タスク・エンジンが nil でも
                      // 安全に呼べるため、`fandhe_apple_mic_record.m` と異なり
                      // 録音実体の有無で分岐する必要が無い、doc 冒頭「バックグラウンド」
                      // 節参照）。
                      FandheSpeechContext *context = g_speech_context;
                      context.interrupted = YES;
                      context.cancelled = YES;
                      FandheSpeechFinish(context, 5, nil);
                    }];
    });
}
#endif

// `dispatch_async(main)` から呼ばれる本体（doc 冒頭「スレッド」節参照）。
static void FandheSpeechStartOnMainThread(uint64_t requestId, double maxSeconds) {
#if TARGET_OS_IPHONE
    FandheSpeechEnsureBackgroundObserver();
#endif

    if (g_speech_context != nil) {
        // 二重開始防御（doc 冒頭参照）。進行中のセッションは潰さない。
        fandhe_rust_speech_finished(requestId, 3, NULL);
        return;
    }

    FandheSpeechContext *context = [[FandheSpeechContext alloc] init];
    context.requestId = requestId;
    g_speech_context = context;

    // マイク権限を明示確認する（doc 冒頭「権限」節参照。拒否時に無音の入力が
    // 届いて「聞き取れず」へ化ける嘘を防ぐ）。
    AVAuthorizationStatus micStatus = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (micStatus == AVAuthorizationStatusDenied || micStatus == AVAuthorizationStatusRestricted) {
        FandheSpeechFinish(context, 1, nil);
        return;
    }

    SFSpeechRecognizerAuthorizationStatus speechStatus = [SFSpeechRecognizer authorizationStatus];
    if (speechStatus == SFSpeechRecognizerAuthorizationStatusDenied
        || speechStatus == SFSpeechRecognizerAuthorizationStatusRestricted) {
        FandheSpeechFinish(context, 1, nil);
        return;
    }

    void (^afterSpeechAuthorized)(void) = ^{
      dispatch_async(dispatch_get_main_queue(), ^{
        if (micStatus == AVAuthorizationStatusNotDetermined) {
          [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                                    completionHandler:^(BOOL granted) {
                                      dispatch_async(dispatch_get_main_queue(), ^{
                                        if (context != g_speech_context) {
                                          return;
                                        }
                                        if (!granted) {
                                          FandheSpeechFinish(context, 1, nil);
                                          return;
                                        }
                                        FandheSpeechBeginListening(context, maxSeconds);
                                      });
                                    }];
        } else {
          FandheSpeechBeginListening(context, maxSeconds);
        }
      });
    };

    if (speechStatus == SFSpeechRecognizerAuthorizationStatusNotDetermined) {
        [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus status) {
          dispatch_async(dispatch_get_main_queue(), ^{
            if (context != g_speech_context) {
              return;
            }
            if (status != SFSpeechRecognizerAuthorizationStatusAuthorized) {
              FandheSpeechFinish(context, 1, nil);
              return;
            }
            afterSpeechAuthorized();
          });
        }];
    } else {
        afterSpeechAuthorized();
    }
}

int32_t fandhe_apple_speech_start(uint64_t request_id, double max_seconds) {
    // Rust の UI スレッドから呼ばれる入口のため `@autoreleasepool` で囲む
    // （device-bridge-contracts.md「FFI 境界」節、`fandhe_apple_mic_record.m` の
    // 対応する入口と同じ方針）。
    @autoreleasepool {
        dispatch_async(dispatch_get_main_queue(), ^{
          FandheSpeechStartOnMainThread(request_id, max_seconds);
        });
    }
    return 0;
}

int32_t fandhe_apple_speech_stop(uint64_t request_id) {
    // Rust の UI スレッドから呼ばれる入口のため `@autoreleasepool` で囲む
    // （device-bridge-contracts.md「FFI 境界」節、`fandhe_apple_speech_start` と
    // 同じ方針）。
    @autoreleasepool {
        dispatch_async(dispatch_get_main_queue(), ^{
          if (g_speech_context == nil || g_speech_context.requestId != request_id) {
              return;
          }
          FandheSpeechContext *context = g_speech_context;
          if (context.task == nil) {
              // まだ聞き始めていない（許可確認中）の停止はキャンセル
              // （doc 冒頭「許可確認中のキャンセル」節参照）。
              context.cancelled = YES;
              FandheSpeechFinish(context, 4, nil);
              return;
          }
          // 話し終わったの合図として、OS に結果の確定を求める（実装計画 §2.2）。
          // マイク入力は即座に遮断する（`endAudio` だけではタップが動き続け、
          // 最終結果または猶予切れまで音声が投入され続けるため、codex 指摘）。
          // 確定は `resultHandler` の `isFinal` 到着、または猶予切れの
          // フォールバックで行う（`FandheSpeechBeginListening` 参照）。
          FandheSpeechStopAudioInput(context);
          dispatch_after(
              dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kFandheSpeechFinishGraceSeconds * NSEC_PER_SEC)),
              dispatch_get_main_queue(), ^{
                if (context != g_speech_context) {
                    return;
                }
                NSString *fallback = context.lastPartialTranscript;
                FandheSpeechFinish(context, 0, fallback != nil ? fallback : @"");
              });
        });
    }
    return 0;
}
