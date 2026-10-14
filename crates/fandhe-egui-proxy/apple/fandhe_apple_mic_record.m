// Phase 11 の拡充ループ（親 #953）型サンプル「マイク録音」: macOS 実装（Issue #954）+
// iOS 実装（Issue #956）。iOS / macOS 共用ファイル
// （`fandhe_apple_location.m`/`fandhe_apple_biometric.m` と同じ判断）。iOS 側は
// `#if TARGET_OS_IPHONE` で以下を追加する:
//   - 録音直前に `AVAudioSession` の category を Record にして `setActive:YES`
//     （マイク使用を OS へ宣言する。macOS の素の実行体には `AVAudioSession` の
//     概念が無いため macOS では no-op）
//   - 完了時（成功・失敗いずれも）に `setActive:NO`
//   - バックグラウンド遷移（`UIApplicationDidEnterBackgroundNotification`）で
//     録音中なら破棄して新設 code=5 で完了通知する（バックグラウンド録音は
//     v1 の範囲外、下記「バックグラウンド」節参照）
// `build.rs` は macOS・iOS 双方の分岐からコンパイルする。
//
// `crates/fandhe-egui-proxy/src/device/apple_mic_record.rs` から C ABI 経由で呼ばれる。
//
// 契約（pull 型。`fandhe_apple_biometric.m` と同型、coding-rust.md の FFI 例外規定）:
//   fandhe_apple_mic_record_start(uint64_t request_id, double max_duration_seconds)
//     -> int32_t 戻り値: 常に 0（依頼を受理し `dispatch_async(main)` へ積んで
//        即座に戻る）。二重開始の判定自体も main queue 上でのみ行う
//        （`g_mic_context` への全アクセスを main queue に閉じる方針、下記
//        「スレッド」節参照）ため、呼び出し元スレッドで同期的に判定しない。
//        結果（成功・失敗・busy のいずれも）は非同期に
//        fandhe_rust_mic_record_finished で 1 回だけ届く。
//   fandhe_apple_mic_record_stop(uint64_t request_id)
//     -> int32_t 戻り値: 0 = 停止依頼を受理した（結果は
//        fandhe_rust_mic_record_finished 経由で届く。多くの場合この呼び出しの
//        中で同期的に届くが、呼び出し元は非同期の完了通知として扱う）。
//        1 = request_id が現在進行中のセッションと一致しない（既に完了済み・
//        取り違え防止のため無視、コールバックは呼ばれない）。
//   fandhe_rust_mic_record_finished(uint64_t request_id, int32_t code,
//                                    const uint8_t *bytes, uint64_t bytes_len,
//                                    const char *mime, uint64_t duration_ms)
//     -> Rust 側（`apple_mic_record.rs`）が公開する `extern "C"` 関数。
//        code: 0 = 成功（bytes/bytes_len/mime/duration_ms が有効）。
//        1 = 権限拒否。2 = 内部エラー（エンコード失敗等）。3 = 二重開始（busy）。
//        4 = 許可確認中に「停止」でキャンセルされた（下記「許可確認中のキャンセル」
//        節参照）。5 = バックグラウンド遷移により破棄された（iOS のみ、下記
//        「バックグラウンド」節参照）。0 以外では bytes/mime を渡さない（NULL）。
//        bytes/mime が指す領域はこの呼び出しが戻るまでのみ有効（ローカル変数）。
//        Rust 側はこの呼び出し内で同期的にコピーし切る契約（`.rs` 側コメント参照）。
//   fandhe_rust_mic_record_started(uint64_t request_id)
//     -> Rust 側が公開する `extern "C"` 関数。`recordForDuration:` が実際に
//        開始した瞬間（`FandheMicRecordBeginRecording` が成功した直後）に 1 回
//        だけ呼ぶ。権限確認待ちの間は呼ばない。Rust 側はこれを見て経過時間の
//        計測を始める（`mark_recording` 自体は「停止」ボタンを出すため権限確認
//        開始時点で先に届く。device-bridge-contracts.md「期限・キャンセル」節・
//        Issue #954 レビュー指摘: 初回の許可ダイアログが遅いと Rust 側の 30 秒
//        予算をダイアログ表示中に消費してしまう問題への対応）。
//
// 権限: `AVCaptureDevice.authorizationStatusForMediaType:` で既知の状態
// （authorized/denied/restricted）を同期判定し、denied/restricted は即座に
// code=1 で完了通知する。notDetermined のときのみ
// `requestAccessForMediaType:completionHandler:` で非同期に確認する
// （complection は任意スレッドから届く、Apple のドキュメント上の契約）。
//
// 録音: 許可が得られたら一時ファイル（NSTemporaryDirectory）へ AAC
// （kAudioFormatMPEG4AAC、モノラル、`.m4a`）で `AVAudioRecorder` を構築し、
// `recordForDuration:` で開始する（OS 側の自動停止、Rust 側 `MAX_RECORD_MS` と
// 同じ値を `max_duration_seconds` で受け取る）。
//
// 停止・完了: `[recorder stop]`（明示停止）と `recordForDuration:` の自動停止は
// いずれも delegate `audioRecorderDidFinishRecording:successfully:` を 1 回だけ
// 呼ぶ（`AVAudioRecorder` の契約）。本実装はこの delegate 1 箇所でのみ
// ファイル読み取り・一時ファイル削除・完了通知を行い、`fandhe_apple_mic_record_stop`
// 自体では読み取りを行わない（二重読み取り・二重削除を避けるため）。
// エンコード失敗（`audioRecorderEncodeErrorDidOccur:`）は code=2 で完了通知する。
//
// 二重開始防御: `g_mic_context != nil` の間に `fandhe_apple_mic_record_start` が
// 呼ばれた場合、進行中のセッションを潰さず、新しい request_id へ code=3（busy）で
// 即座に完了通知する。通常は Rust 側の状態機械（`Idle` 以外はボタン非表示）が
// 二重要求自体を防ぐため、この分岐は防御的コードに留まる
// （`fandhe_apple_biometric.m` の同種の分岐と同じ位置づけ）。
//
// スレッド: 開始・停止依頼はいずれも `dispatch_async(main)` で実処理する
// （`fandhe_apple_biometric.m`/`fandhe_apple_share.m` と同じ方針。`g_mic_context`
// への全アクセスを main queue に閉じ、形式的なデータレースも作らない）。
//
// 許可確認中のキャンセル（device-bridge-contracts.md 観点 1「ライフサイクル境界」・
// 観点 3「期限・キャンセル」、Issue #954 レビュー指摘）: `g_mic_context` は
// 「録音中」だけでなく「許可確認中（`requestAccessForMediaType:` の完了待ち）」も
// 含めて 1 つのワーカー世代として `FandheMicRecordStartOnMainThread` の冒頭で
// 予約する（`AVAudioRecorder` 自体は許可が下りてから生成するため、以前は
// この間 `g_mic_context` が nil のままで `fandhe_apple_mic_record_stop` から見えず、
// 停止依頼が握り潰されていた）。`fandhe_apple_mic_record_stop` は
// `g_mic_recorder`（実際の録音）がまだ nil ならこの予約段階と判断し、
// delegate の `cancelled` を立てて即座に code=4 で完了通知する。
// `requestAccessForMediaType:` の completion（`delegate` を直接キャプチャする）は
// 許可が下りても `cancelled` を確認してから録音を開始する
// （`FandheMicRecordBeginRecording` 冒頭）ため、キャンセル後に許可が下りても
// マイクは起動しない。
//
// 情報非保持: 録音データ本体以外（マイクの入力レベル・端末識別子等）は Rust 側へ
// 渡さない。`NSLog` を含むログ出力は一切行わない（security.md A09）。
//
// AVAudioSession（iOS のみ、Issue #956）: 録音直前に category を
// `AVAudioSessionCategoryRecord` にして `setActive:YES` する。macOS の素の
// 実行体には `AVAudioSession` の概念が無いため、macOS では no-op マクロに
// している（TARGET_OS_IPHONE で分岐）。有効化前の category / categoryOptions
// を保存し、完了時（成功・失敗いずれも、`finishWithCode:recorder:` 内）に
// `setActive:NO` して解放したうえで両方を復元する（他アプリへの通知が目的で
// 失敗は無視する。codex-review #961 P1 指摘: categoryOptions を保存せず戻すと
// 既定値へ置き換わり、他機能が設定した
// `AVAudioSessionCategoryOptionMixWithOthers` 等が録音後に失われる）。
//
// バックグラウンド（iOS のみ、Issue #956）: `UIApplicationDidEnterBackgroundNotification`
// を購読し（`fandhe_apple_gamepad.m` の `dispatch_once` 購読と同じ書式）、
// 録音中（`g_mic_recorder != nil`）ならセッションを破棄して code=5 で完了通知する
// （v1 の天井。バックグラウンド録音・割り込みからの復帰再開は範囲外、
// device-bridge-contracts.md「対応表・権限」節参照）。許可確認中
// （`g_mic_recorder == nil`）に到達した場合も同じ天井を守るため、直ちに
// code=5 で完了通知して世代を解放する（システムの許可ダイアログは通常
// DidEnterBackground を発火しないため稀な経路だが、発火時に放置すると
// `granted` completion がバックグラウンドのまま録音を開始してしまうため、
// codex-review #961 P1 指摘を受けて対応した）。

#import <TargetConditionals.h>
#import <AVFoundation/AVFoundation.h>
#import <AudioToolbox/AudioToolbox.h>
#import <mach/mach_time.h>
#import <math.h>
#if TARGET_OS_IPHONE
#import <UIKit/UIKit.h>
#endif

// 録音時間の計測には壁時計（`NSDate`）を使わない（システム時刻の変更・時刻同期補正で
// 差分が負や異常値になり得るため、codex-review #959 P1 指摘・
// device-bridge-contracts.md「嘘の成功」節）。`mach_continuous_time` は単調増加かつ
// スリープ時間も含むため、実測の録音時間として扱える。フレームワーク追加リンクが
// 不要（libSystem 提供）な点も `CACurrentMediaTime`（要 QuartzCore）より軽い。
static uint64_t FandheMicRecordMonotonicNow(void) {
    return mach_continuous_time();
}

// `mach_continuous_time` の tick 数をミリ秒へ変換する（`mach_timebase_info` で
// tick → ナノ秒の比を取得。ホストによって比が異なるため実行時に問い合わせる契約）。
static uint64_t FandheMicRecordTicksToMillis(uint64_t ticks) {
    static mach_timebase_info_data_t timebase;
    if (timebase.denom == 0) {
        mach_timebase_info(&timebase);
    }
    long double nanos = (long double)ticks * (long double)timebase.numer / (long double)timebase.denom;
    return (uint64_t)(nanos / 1000000.0L);
}

// Rust 側（`apple_mic_record.rs`）が公開する `extern "C"` 関数。
extern void fandhe_rust_mic_record_finished(uint64_t request_id,
                                             int32_t code,
                                             const uint8_t *bytes,
                                             uint64_t bytes_len,
                                             const char *mime,
                                             uint64_t duration_ms);

// `recordForDuration:` が実際に開始した瞬間に呼ぶ（Rust 側が権限確認中の待ち時間を
// 録音時間として数え始めないための合図、device-bridge-contracts.md「期限・
// キャンセル」節・Issue #954 レビュー指摘）。`.rs` 側が
// `device/mic_record::bridge` へ「実際に開始した request_id」を記録する。
extern void fandhe_rust_mic_record_started(uint64_t request_id);

// delegate が reply を待つ間・ファイルの後始末を行う間、プロセス内で生存させる
// 必要があるため static で強参照する（下記 `g_mic_context` 参照）。
@interface FandheMicRecordDelegate : NSObject <AVAudioRecorderDelegate>
@property(nonatomic, assign) uint64_t requestId;
// `AVAudioRecorder` 自体は開始時刻を公開しないため、実測時間の算出用に
// delegate 側で開始時刻を保持する（`stop` 後は `recorder.currentTime` が
// リセットされうるため、`currentTime` を信用しない）。壁時計（`NSDate`）は
// システム時刻変更・同期補正で異常値になり得るため、単調増加時計
// （`mach_continuous_time`）の tick 値で保持する（codex-review #959 P1 指摘）。
@property(nonatomic, assign) uint64_t startedAtTicks;
// 許可確認中に「停止」が押されたことを示す（上記「許可確認中のキャンセル」節
// 参照）。true のときは許可が下りても `FandheMicRecordBeginRecording` は録音を
// 開始しない（`fandhe_apple_mic_record_stop` が既に code=4 で完了通知済みのため、
// 二重通知を避ける）。
@property(nonatomic, assign) BOOL cancelled;
// `finishWithCode:recorder:` の二重実行防御（`AVAudioRecorder` の
// `audioRecorderDidFinishRecording:successfully:`/`audioRecorderEncodeErrorDidOccur:`
// が同一セッションで両方届いた場合に、2 回目の完了通知・共有状態の二重解放を
// 防ぐ。1 回目で YES を立て、2 回目以降は即座に return する
// （codex-review #954 P1 指摘・device-bridge-contracts.md「FFI 境界」節）。
@property(nonatomic, assign) BOOL finished;
// バックグラウンド遷移（iOS のみ）で破棄されたことを示す（doc 冒頭
// 「バックグラウンド」節参照）。true のときは `finishWithCode:recorder:` が
// code を 5 へ上書きする（`audioRecorderDidFinishRecording:` が成功として
// 届いても破棄として扱うため）。
@property(nonatomic, assign) BOOL interrupted;
// `finishWithCode:recorder:` が Inactive 猶予後に実処理へ渡すための内部メソッド
// （下記 `finalizeWithCode:recorder:` の doc 参照）。
- (void)finalizeWithCode:(int32_t)code recorder:(AVAudioRecorder *)recorder;
@end

#if TARGET_OS_IPHONE
// `finishWithCode:recorder:` が Inactive を検出したときの猶予秒数（doc 冒頭
// 「バックグラウンド」節・codex-review #961 P1 再指摘）。Control Center・
// 通知センター・システム警告等フォアグラウンドのままの一時的な Inactive は
// 通常 1 秒未満で Active に戻るため、実際にバックグラウンドへ遷移する場合との
// 判別に十分な長さとして 0.5 秒とした（実測に基づく調整可能な値、固定契約では
// ない）。
static const NSTimeInterval kFandheMicRecordInactiveGraceSeconds = 0.5;
#endif

// 進行中セッション（`AVAudioRecorder` + delegate）。`g_mic_context` が nil の
// ときのみ新規セッションを開始できる（二重開始防御、上記コメント参照）。
// 全アクセスを main queue に閉じる（上記「スレッド」節参照）。
static AVAudioRecorder *g_mic_recorder = nil;
static FandheMicRecordDelegate *g_mic_context = nil;

#if TARGET_OS_IPHONE
// AVAudioSession を Record カテゴリで有効化する（doc 冒頭「AVAudioSession」節
// 参照）。失敗（他アプリとの競合等）は NO を返し、呼び出し元が code=2 で
// 完了通知する。
// 有効化前の category / categoryOptions を記憶し、解放時に復元する（doc 冒頭
// 「AVAudioSession」節・Bugbot #961 指摘: Record カテゴリのまま放置すると、
// 後続の再生（`AVSpeechSynthesizer` 等、独自にカテゴリを設定しない経路）が
// 無音化する）。categoryOptions も合わせて復元する（codex-review #961 P1 指摘:
// options（例: `AVAudioSessionCategoryOptionMixWithOthers`）を保存せず
// `setCategory:error:`（options なし版）で戻すと既定値へ置き換わり、他アプリの
// 音声を止めない設定だった呼び出し元の挙動が録音後に変化する）。
// セッションは単一グローバル（`g_mic_recorder` と同様に同時に 1 件のみ）のため
// static 変数で足りる。
static NSString *g_mic_previous_category = nil;
static AVAudioSessionCategoryOptions g_mic_previous_category_options = 0;

static BOOL FandheMicRecordActivateSession(void) {
    NSError *error = nil;
    AVAudioSession *session = [AVAudioSession sharedInstance];
    NSString *previousCategory = session.category;
    AVAudioSessionCategoryOptions previousCategoryOptions = session.categoryOptions;
    if (![session setCategory:AVAudioSessionCategoryRecord error:&error]) {
        return NO;
    }
    if (![session setActive:YES error:&error]) {
        // category の変更だけ先に成功しているため、有効化に失敗した場合は
        // 元の category / categoryOptions へ戻してから NO を返す
        // （codex/Bugbot #961 指摘: ここで戻さないと Record のまま共有セッションに
        // 残り、後続の AVSpeechSynthesizer 等の再生が無音化する）。
        [session setCategory:previousCategory withOptions:previousCategoryOptions error:nil];
        return NO;
    }
    g_mic_previous_category = previousCategory;
    g_mic_previous_category_options = previousCategoryOptions;
    return YES;
}

// セッションを解放し、有効化前の category / categoryOptions へ戻す（doc 冒頭
// 「AVAudioSession」節参照）。録音は既に終わっており他アプリへ通知するのが目的の
// ため、失敗は無視する。
//
// 本関数は共有セッションを無条件に `setActive:NO` する（codex-review #961 P1
// 指摘、オーナー判断で現状維持）。他の音声 I/O が動作中は iOS 側が
// `AVAudioSessionErrorCodeIsBusy` で拒否しセッションは有効なまま残るため、
// TTS（`AVSpeechSynthesizer`、自前でセッションを再有効化する）発話中に録音を
// 終えても発話は止まらない。本リポジトリで `AVAudioSession` を直接扱うのは
// 本ファイルのみ。共有セッションを触る 2 つ目の機能を足す時点で、所有権の
// 一元管理（誰が有効化したかを記録し、自分が有効化した場合のみ解放する）が
// 必要になる。
static void FandheMicRecordDeactivateSession(void) {
    AVAudioSession *session = [AVAudioSession sharedInstance];
    [session setActive:NO
           withOptions:AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation
                 error:nil];
    if (g_mic_previous_category != nil) {
        [session setCategory:g_mic_previous_category
                  withOptions:g_mic_previous_category_options
                        error:nil];
        g_mic_previous_category = nil;
        g_mic_previous_category_options = 0;
    }
}
#else
// macOS の素の実行体には `AVAudioSession` の概念が無いため no-op にする
// （doc 冒頭「AVAudioSession」節参照）。
static BOOL FandheMicRecordActivateSession(void) {
    return YES;
}
static void FandheMicRecordDeactivateSession(void) {
}
#endif

@implementation FandheMicRecordDelegate

- (void)finishWithCode:(int32_t)code recorder:(AVAudioRecorder *)recorder {
    // 二重実行防御（`finished` プロパティの doc 参照）。1 回目で立てて即座に
    // return しない（後続の分岐がまだこの呼び出しを完了させる必要がある）ため
    // ガードのみここで行う。
    if (self.finished) {
        return;
    }
    self.finished = YES;

    // バックグラウンド遷移で破棄済み（doc 冒頭「バックグラウンド」節参照）なら、
    // delegate が成功として届けても破棄として扱う（`code` を上書きする）。
    if (self.interrupted) {
        code = 5;
    }
#if TARGET_OS_IPHONE
    // `UIApplicationDidEnterBackgroundNotification` は AVAudioSession の中断に
    // 起因する delegate の成功終了コールバック（既に main queue へ
    // dispatch_async 済み）より後に届く場合がある。この場合 `self.interrupted`
    // はまだ立っておらず、本来 code=5 とすべき打ち切り録音が code=0 の成功として
    // Rust 側へ渡ってしまう（Bugbot #961 Medium 指摘）。両者は queue 上の到着
    // 順序に依存するため、通知の到着順ではなく現在のアプリ状態を都度確認する
    // ことでこの競合を解消する。
    //
    // バックグラウンド遷移はまず `UIApplicationStateInactive` を経てから
    // `UIApplicationStateBackground` に進む。前巡（codex-review #961 P1）は
    // `== UIApplicationStateBackground` 比較を指摘され「Inactive 期間中に届く
    // 成功コールバックを取りこぼす」ため Active 以外を全て破棄にしたが、今巡
    // （codex-review #961 P1 再指摘）は逆に「Control Center・通知センター等
    // フォアグラウンドのままの一時的な Inactive でも、その間に成功完了が届くと
    // バックグラウンドへ実際には遷移していないのに破棄される」と指摘された。
    // 契約（doc 冒頭「バックグラウンド」節・対応表）は
    // `UIApplicationDidEnterBackgroundNotification` による遷移のみを破棄条件と
    // するため、Inactive を即座に破棄扱いにはできない。
    //
    // Background は即座に破棄。Inactive は `finalizeWithCode:recorder:` の
    // 呼び出しを `kFandheMicRecordInactiveGraceSeconds` だけ猶予し、猶予後に
    // `self.interrupted`（`UIApplicationDidEnterBackgroundNotification` 経由で
    // 立つ）か `applicationState == Background` のいずれかが真なら破棄、
    // そうでなければ成功として確定する。猶予中に本物のバックグラウンド遷移が
    // 起きれば通知が `interrupted` を立てるため、到達順に依存せず判定できる。
    UIApplicationState state = [UIApplication sharedApplication].applicationState;
    if (code == 0 && state == UIApplicationStateBackground) {
        code = 5;
    } else if (code == 0 && state == UIApplicationStateInactive) {
        dispatch_after(
            dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kFandheMicRecordInactiveGraceSeconds * NSEC_PER_SEC)),
            dispatch_get_main_queue(), ^{
              int32_t finalCode = 0;
              if (self.interrupted
                  || [UIApplication sharedApplication].applicationState == UIApplicationStateBackground) {
                  finalCode = 5;
              }
              [self finalizeWithCode:finalCode recorder:recorder];
            });
        return;
    }
#endif

    [self finalizeWithCode:code recorder:recorder];
}

// `finishWithCode:recorder:` から即座に、または Inactive 猶予（上記参照）を
// 経てから 1 回だけ呼ばれる。完了通知・ファイル削除・セッション解放の実処理。
- (void)finalizeWithCode:(int32_t)code recorder:(AVAudioRecorder *)recorder {
    uint64_t requestId = self.requestId;
    NSURL *fileURL = recorder.url;

    // 次のセッションが既に `g_mic_context` を上書きしている可能性があるため
    // （`finished` ガードにより通常はここへ二重到達しないが、識別子確認を
    // 独立した安全網として残す）、自分が現在のセッションのときだけ共有状態を
    // 解放する。他方 `fandhe_rust_mic_record_finished` への完了通知自体は
    // 常に行う（レビュー指摘の「完了の欠落」を避ける）。
    BOOL isCurrentSession = (g_mic_context == self);

    if (code != 0) {
        [[NSFileManager defaultManager] removeItemAtURL:fileURL error:nil];
        FandheMicRecordDeactivateSession();
        fandhe_rust_mic_record_finished(requestId, code, NULL, 0, NULL, 0);
        if (isCurrentSession) {
            g_mic_context = nil;
            g_mic_recorder = nil;
        }
        return;
    }

    NSData *data = [NSData dataWithContentsOfURL:fileURL];
    [[NSFileManager defaultManager] removeItemAtURL:fileURL error:nil];
    if (data == nil) {
        FandheMicRecordDeactivateSession();
        fandhe_rust_mic_record_finished(requestId, 2, NULL, 0, NULL, 0);
        if (isCurrentSession) {
            g_mic_context = nil;
            g_mic_recorder = nil;
        }
        return;
    }

    // `recorder.currentTime` は停止後にリセットされうるため、開始時刻
    // （`startedAtTicks`）との差分から実測時間を求める（`.h` に持たない
    // シンプルな associated object の代わりに、delegate 自身のプロパティで保持）。
    // 単調増加時計のため引き算が負になることはない（壁時計と異なりシステム時刻
    // 変更の影響を受けない、codex-review #959 P1 指摘）。
    uint64_t durationMs = FandheMicRecordTicksToMillis(FandheMicRecordMonotonicNow() - self.startedAtTicks);

    FandheMicRecordDeactivateSession();
    fandhe_rust_mic_record_finished(requestId, 0, (const uint8_t *)data.bytes,
                                     (uint64_t)data.length, "audio/mp4", durationMs);
    if (isCurrentSession) {
        g_mic_context = nil;
        g_mic_recorder = nil;
    }
}

- (void)audioRecorderDidFinishRecording:(AVAudioRecorder *)recorder successfully:(BOOL)flag {
    // delegate callback はメインスレッド外から届き得るため main queue へ戻す
    // （`g_mic_context`/`g_mic_recorder` への全アクセスを main queue に閉じる
    // 方針、doc 冒頭「スレッド」節・Bugbot #954 指摘）。
    dispatch_async(dispatch_get_main_queue(), ^{
      [self finishWithCode:(flag ? 0 : 2) recorder:recorder];
    });
}

- (void)audioRecorderEncodeErrorDidOccur:(AVAudioRecorder *)recorder error:(NSError *)error {
    dispatch_async(dispatch_get_main_queue(), ^{
      [self finishWithCode:2 recorder:recorder];
    });
}

@end

// `delegate` は `FandheMicRecordStartOnMainThread` が既に `g_mic_context` として
// 予約済みのもの（上記「許可確認中のキャンセル」節参照）。ここで新規に確保しない
// ことで、許可確認中に立った `cancelled` を録音開始直前に確認できる。
static void FandheMicRecordBeginRecording(FandheMicRecordDelegate *delegate, double maxDurationSeconds) {
    uint64_t requestId = delegate.requestId;

    if (delegate.cancelled) {
        // 許可確認中に「停止」でキャンセル済み（`fandhe_apple_mic_record_stop` が
        // 既に code=4 で完了通知・`g_mic_context` を解放済み）。録音を開始せず、
        // ここでは何もしない（二重の完了通知を避ける）。
        return;
    }

    // iOS のみ、マイク使用を OS へ宣言する（doc 冒頭「AVAudioSession」節参照）。
    // macOS は no-op。一時ファイルをまだ作成していないため失敗時の削除は不要。
    if (!FandheMicRecordActivateSession()) {
        fandhe_rust_mic_record_finished(requestId, 2, NULL, 0, NULL, 0);
        g_mic_context = nil;
        return;
    }

    NSString *path = [NSTemporaryDirectory()
        stringByAppendingPathComponent:[NSString stringWithFormat:@"fandhe-mic-%llu.m4a", requestId]];
    NSURL *url = [NSURL fileURLWithPath:path];

    NSDictionary *settings = @{
        AVFormatIDKey : @(kAudioFormatMPEG4AAC),
        AVNumberOfChannelsKey : @1,
        AVSampleRateKey : @44100.0,
    };

    NSError *error = nil;
    AVAudioRecorder *recorder = [[AVAudioRecorder alloc] initWithURL:url settings:settings error:&error];
    if (recorder == nil) {
        // `initWithURL:settings:error:` が失敗しても一時ファイルが作成されている
        // 場合があるため削除する（録音データは機微情報、codex-review #959 P1
        // 指摘・device-bridge-contracts.md「秘密値」節）。存在しなければ no-op。
        [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
        FandheMicRecordDeactivateSession();
        fandhe_rust_mic_record_finished(requestId, 2, NULL, 0, NULL, 0);
        g_mic_context = nil;
        return;
    }

    delegate.startedAtTicks = FandheMicRecordMonotonicNow();
    recorder.delegate = delegate;

    if (![recorder recordForDuration:maxDurationSeconds]) {
        // `recordForDuration:` 失敗時も同様に一時ファイルを削除する
        // （codex-review #959 P1 指摘）。
        [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
        FandheMicRecordDeactivateSession();
        fandhe_rust_mic_record_finished(requestId, 2, NULL, 0, NULL, 0);
        g_mic_context = nil;
        return;
    }

    g_mic_recorder = recorder;
    // 権限確認の待ち時間を録音時間として数えないよう、実際に録音が始まった
    // 瞬間を Rust 側へ知らせる（doc 冒頭の `fandhe_rust_mic_record_started`
    // 節参照）。
    fandhe_rust_mic_record_started(requestId);
}

#if TARGET_OS_IPHONE
// バックグラウンド遷移の購読を 1 回だけ行う（doc 冒頭「バックグラウンド」節・
// `fandhe_apple_gamepad.m` の `dispatch_once` 購読と同じ書式）。ハンドラは
// main queue 上で呼ばれる（`addObserverForName:` の `queue:` 引数）ため、
// `g_mic_context`/`g_mic_recorder` への全アクセスを main queue に閉じる方針
// （doc 冒頭「スレッド」節）を保てる。
// `addObserverForName:...usingBlock:` の戻り値（observer token）は呼び出し元が
// 強参照し続けない限り ARC が即座に解放し、その dealloc で購読自体が
// 解除される（NSNotificationCenter は登録時にこのトークンを強く保持しない）。
// 戻り値を捨てていた旧実装ではバックグラウンド遷移の通知が一切届かず、
// code=5 の破棄パスが機能していなかった（Bugbot #961 High 指摘）。
// プロセス生存中ずっと購読させたいため static で永続的に保持する。
static id g_mic_background_observer = nil;

static void FandheMicRecordEnsureBackgroundObserver(void) {
    static dispatch_once_t once_token;
    dispatch_once(&once_token, ^{
        g_mic_background_observer = [[NSNotificationCenter defaultCenter]
            addObserverForName:UIApplicationDidEnterBackgroundNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *_Nonnull __unused note) {
                      if (g_mic_context == nil) {
                          // 進行中セッションなし。何もしていない。
                          return;
                      }
                      if (g_mic_recorder != nil) {
                          // 録音中: 破棄扱いにしてから停止する。delegate 経由で
                          // 1 回だけ完了通知・ファイル削除・session 解放が届く
                          // （`finishWithCode:recorder:` の `interrupted` 分岐）。
                          g_mic_context.interrupted = YES;
                          [g_mic_recorder stop];
                          return;
                      }
                      // 許可確認中: システムの許可ダイアログは通常
                      // DidEnterBackground を発火しないため稀な経路だが、
                      // 発火した場合に何もしないと `granted` completion が
                      // バックグラウンドのまま録音を開始してしまう
                      // （v1 の「バックグラウンド録音は範囲外」契約に反する、
                      // codex-review #961 P1 指摘）。`fandhe_apple_mic_record_stop`
                      // の許可確認中キャンセル分岐と同じ手順で、直ちに破棄扱いの
                      // 完了通知（code=5）を送って世代を解放する。completion は
                      // `delegate.cancelled` を見て録音開始を取りやめる
                      // （`FandheMicRecordBeginRecording` 冒頭）ため、この後に
                      // 許可が下りてもマイクは起動しない。
                      FandheMicRecordDelegate *pending = g_mic_context;
                      pending.cancelled = YES;
                      g_mic_context = nil;
                      fandhe_rust_mic_record_finished(pending.requestId, 5, NULL, 0, NULL, 0);
                    }];
    });
}
#endif

static void FandheMicRecordStartOnMainThread(uint64_t requestId, double maxDurationSeconds) {
#if TARGET_OS_IPHONE
    FandheMicRecordEnsureBackgroundObserver();
#endif

    if (g_mic_context != nil) {
        // 進行中のセッションを潰さない（doc の「二重開始防御」参照）。
        fandhe_rust_mic_record_finished(requestId, 3, NULL, 0, NULL, 0);
        return;
    }

    // 許可確認中（この関数の残り）も含めて 1 つのワーカー世代として予約する
    // （上記「許可確認中のキャンセル」節参照）。
    FandheMicRecordDelegate *delegate = [[FandheMicRecordDelegate alloc] init];
    delegate.requestId = requestId;
    g_mic_context = delegate;

    AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (status == AVAuthorizationStatusDenied || status == AVAuthorizationStatusRestricted) {
        fandhe_rust_mic_record_finished(requestId, 1, NULL, 0, NULL, 0);
        g_mic_context = nil;
        return;
    }
    if (status == AVAuthorizationStatusAuthorized) {
        FandheMicRecordBeginRecording(delegate, maxDurationSeconds);
        return;
    }

    // notDetermined: 初回のみ許可プロンプトが出る。completion は任意スレッドから
    // 届くため、実処理（録音開始）は main queue へ戻す（`g_mic_context` への
    // 全アクセスを main queue に閉じる方針、doc 冒頭参照）。`delegate` はブロックが
    // 直接キャプチャする（`g_mic_context` 経由ではない）ため、キャンセルにより
    // `g_mic_context` が別の値（nil や次のセッション）へ進んでいても、この
    // completion は自分が予約した世代の `cancelled` を正しく確認できる。
    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                             completionHandler:^(BOOL granted) {
                               dispatch_async(dispatch_get_main_queue(), ^{
                                 if (delegate.cancelled) {
                                     // 既に `fandhe_apple_mic_record_stop` が
                                     // code=4 で完了通知済み（二重通知を避け
                                     // ここでは何もしない）。
                                     return;
                                 }
                                 if (granted) {
                                     FandheMicRecordBeginRecording(delegate, maxDurationSeconds);
                                 } else {
                                     fandhe_rust_mic_record_finished(requestId, 1, NULL, 0, NULL, 0);
                                     g_mic_context = nil;
                                 }
                               });
                             }];
}

int32_t fandhe_apple_mic_record_start(uint64_t request_id, double max_duration_seconds) {
    dispatch_async(dispatch_get_main_queue(), ^{
      FandheMicRecordStartOnMainThread(request_id, max_duration_seconds);
    });
    return 0;
}

int32_t fandhe_apple_mic_record_stop(uint64_t request_id) {
    dispatch_async(dispatch_get_main_queue(), ^{
      if (g_mic_context == nil || g_mic_context.requestId != request_id) {
          return;
      }
      if (g_mic_recorder != nil) {
          // `stop` は delegate の `audioRecorderDidFinishRecording:successfully:YES`
          // を呼ぶ（`AVAudioRecorder` の契約）。実際の読み取り・完了通知は
          // delegate 側の 1 箇所（`finishWithCode:recorder:`）に一本化してある
          // （doc 冒頭「停止・完了」節参照）。
          [g_mic_recorder stop];
          return;
      }
      // `g_mic_recorder` がまだ nil ＝ 許可確認中（上記「許可確認中のキャンセル」
      // 節参照）。ここで即座にキャンセル済みとして完了通知し、`g_mic_context` を
      // 解放する。許可確認の completion は `delegate.cancelled` を見て録音開始を
      // 取りやめる（`FandheMicRecordBeginRecording` 冒頭）。
      FandheMicRecordDelegate *pending = g_mic_context;
      pending.cancelled = YES;
      g_mic_context = nil;
      fandhe_rust_mic_record_finished(pending.requestId, 4, NULL, 0, NULL, 0);
    });
    return 0;
}
