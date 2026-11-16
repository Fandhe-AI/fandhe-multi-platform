// Phase 11 の拡充ループ（親 #1042）型サンプル「バックグラウンド処理」の iOS 実装
// （Issue #1045）。macOS（#1043、`NSBackgroundActivityScheduler`）は macOS 専用 API の
// ため使えず、iOS は `BackgroundTasks`（`BGTaskScheduler`）を使う
// （`macos/fandhe_macos_background_task.m` 冒頭コメント参照）。
//
// `crates/fandhe-egui-proxy/src/device/ios_background_task.rs` から C ABI 経由で
// 呼ばれる。`build.rs` が feature `device` 有効時のみ `cc` コンパイルし、
// BackgroundTasks をリンクする。
//
// 契約（coding-rust.md の FFI 例外規定、`fandhe_apple_biometric.m` と同型）:
//   int32_t fandhe_ios_background_task_register(void)
//     -> main thread から起動完了前（UIApplicationMain 呼び出し前）に 1 回だけ呼ぶ
//        契約（Apple ドキュメント要件、起動後の登録はできない）。
//        `registerForTaskWithIdentifier:usingQueue:launchHandler:` を呼び、
//        戻り値の BOOL を `g_registered` に保存する。0 = 成功、1 = 失敗。
//        launchHandler が実際に OS から呼ばれるのはこの関数が返った後（非同期）。
//   int32_t fandhe_ios_background_task_submit(double earliest_begin_secs)
//     -> 未登録（`g_registered == NO`）のまま呼ばない契約（呼び出し元
//        `ios_background_task.rs::IosBackgroundTask::schedule` が `instance()` 経由で
//        1 回だけ register 済みの状態を前提にする）。ハンドラ未登録の予約が発火すると
//        クラッシュするため、未登録時は何もせず 3 を返す。
//        戻り値:
//          0 = 受理（`submitTaskRequest:error:` が成功）
//          1 = `BGTaskSchedulerErrorCodeUnavailable`（バックグラウンド更新が無効
//              または省電力モード）
//          2 = `BGTaskSchedulerErrorCodeNotPermitted`（Info.plist に identifier 未宣言）
//          3 = 非対応（シミュレータ、または未登録）
//          4 = その他（上限超過・未知のコード・別 domain・request 生成失敗）
//        コードの分類はここで行うが、`DeviceError` への写像自体は Rust 側
//        `device/background_task.rs::submit_code_to_result`（SSOT）に一本化する
//        （`.m` 側で二重に写像しない、`fandhe_ios_notification.m` と同じ方針）。
//   void fandhe_ios_background_task_cancel(void)
//     -> `cancelTaskRequestWithIdentifier:` を呼ぶ。未予約でも安全（べき等）。
//   int32_t fandhe_rust_ios_background_task_fired(int32_t result)
//     -> Rust 側（`ios_background_task.rs`）が公開する `extern "C"` 関数。
//        result: 0 = Completed、1 = Expired。戻り値: 0 = 保存成功、1 = 失敗。
//
// launch handler のスレッド: `g_task_queue`（専用シリアルキュー、メインキューではない）を
// 使う契約（Apple ドキュメント「`usingQueue:` は launchHandler と `expirationHandler` の
// 両方を呼ぶキュー」、直上コメント参照）。block 内は `@autoreleasepool` で囲む
// （device-bridge-contracts.md「5. FFI 境界」）。
//
// # 完了経路の単純化（オーナー承認済み方針、2026-10-02、#1050 未解決スレッド 3 件への対処）
//
// - 期限切れ経路（`expirationHandler`）は、まだ誰も完了を確定していなければ
//   `Expired` を保存してから `setTaskCompletedWithSuccess:NO` を呼ぶ（完了通知後は
//   プロセスが止まり得るため保存を後回しにしない。PR #1050 codex 指摘）
// - 通常完了経路（launchHandler 本体）は `g_work_queue` で保存し、まだ確定して
//   いなければ保存の成否で完了を通知する。既に期限切れで確定済みなら保存しない
// - 通常完了の保存は専用シリアルキュー `g_work_queue` 上で行い、OS が
//   `expirationHandler` を呼ぶキュー（`g_task_queue`）を塞がない。期限切れ側の保存と
//   並行しても、Rust 側 `RECORD_LOCK` が比較と書き込みを直列化する
// - 一度きりの完了呼び出しは `finished`（`@synchronized(task)` の瞬時の
//   check-and-set、保存処理を含まない）で保証する（device-bridge-contracts.md
//   「5. FFI 境界」completion の一度きり契約）
// - 記録の最終的な内容は Rust 側 `ios_background_task.rs::on_fired` の「時刻の新しい方を
//   残す」規則で決まる

// 識別子は Info.plist の `BGTaskSchedulerPermittedIdentifiers` と一致させる
// （この定数 1 箇所と Info.plist の 2 か所でのみ定義、相互参照コメントあり）。
//
// 情報非保持: `NSError` の文言をログへ出力しない（security.md A09）。

#import <BackgroundTasks/BackgroundTasks.h>
#import <TargetConditionals.h>
#import <dispatch/dispatch.h>

// 識別子: `platforms/apple/FandheIosApp/Resources/Info.plist` の
// `BGTaskSchedulerPermittedIdentifiers` と一致させる（相互参照コメントあり）。
static NSString *const kFandheBackgroundTaskIdentifier = @"com.fandhe.iosapp.background-task";

// Rust 側（`ios_background_task.rs`）が公開する `extern "C"` 関数。
extern int32_t fandhe_rust_ios_background_task_fired(int32_t result);

// `register()` が成功したかどうか。未登録のまま `submit` を呼ぶとハンドラ無しの
// 予約が発火してクラッシュするため、`submit` 側のガードに使う（ファイル冒頭
// コメント参照）。シミュレータでは `registerForTaskWithIdentifier:` 自体は
// 成功を返すことがあるため、`TARGET_OS_SIMULATOR` も別途 `submit` でガードする。
static BOOL g_registered = NO;

// launchHandler と `expirationHandler` の両方をこのキュー上で OS が呼ぶ契約
// （`registerForTaskWithIdentifier:usingQueue:`）。保存処理をここで走らせないことで
// `expirationHandler` がいつでも空いているこのキュー上で即座に実行できるようにする
// （ファイル冒頭コメント「完了経路の単純化」参照）。
static dispatch_queue_t g_task_queue;

// 通常完了経路（保存を伴う）専用のキュー。`g_task_queue` とは別にすることで、
// 保存処理が `expirationHandler` の実行をブロックしないようにする
// （ファイル冒頭コメント参照）。
static dispatch_queue_t g_work_queue;

int32_t fandhe_ios_background_task_register(void) {
  g_task_queue = dispatch_queue_create("com.fandhe.iosapp.background-task", DISPATCH_QUEUE_SERIAL);
  g_work_queue =
      dispatch_queue_create("com.fandhe.iosapp.background-task-work", DISPATCH_QUEUE_SERIAL);
  BOOL ok = [[BGTaskScheduler sharedScheduler]
      registerForTaskWithIdentifier:kFandheBackgroundTaskIdentifier
                          usingQueue:g_task_queue
                       launchHandler:^(BGTask *task) {
                         // SAFETY（ファイル冒頭コメント「launch handler のスレッド」
                         // 参照）: OS の内部キューから届く。`finished` は
                         // `setTaskCompletedWithSuccess:` の一度きり呼び出しのみを
                         // 保護する check-and-set で、保存処理は待たない
                         // （ファイル冒頭コメント「完了経路の単純化」参照）。
                         __block BOOL finished = NO;
                         // `task` は `expirationHandler` を保持するため、handler からは
                         // 弱参照で触る（強参照すると循環してタスクが解放されない。
                         // PR #1050 codex 指摘）。
                         __weak BGTask *weakTask = task;
                         task.expirationHandler = ^{
                           // 期限切れ経路: `Expired` を保存してから完了を通知する。完了
                           // 通知後は OS がプロセスを止め得るため、保存を通知後の非同期
                           // 処理に頼らない（PR #1050 codex 指摘。データの消失を避ける
                           // ことを、通知を数ミリ秒早めることより優先する。保存は 60 バイト
                           // 程度の 1 回の書き込みで、Rust 側 `RECORD_LOCK` の待ちも同程度）。
                           BGTask *task = weakTask;
                           if (task == nil) {
                             return;
                           }
                           BOOL should_finish;
                           @synchronized(task) {
                             should_finish = !finished;
                             finished = YES;
                           }
                           if (should_finish) {
                             @autoreleasepool {
                               (void)fandhe_rust_ios_background_task_fired(1);
                             }
                             [task setTaskCompletedWithSuccess:NO];
                           }
                         };
                         // 宛先は `g_work_queue`（`g_task_queue` とは別のキュー）で、
                         // OS が `expirationHandler` を呼ぶ `g_task_queue` を保存処理で
                         // ブロックしない（ファイル冒頭コメント参照）。
                         dispatch_async(g_work_queue, ^{
                           // 通常完了経路: 既に期限切れで完了済みなら保存しない
                           // （期限切れ側が `Expired` を同じキューへ積む）。
                           BOOL already_finished;
                           @synchronized(task) {
                             already_finished = finished;
                           }
                           if (already_finished) {
                             return;
                           }
                           int32_t save_ok;
                           @autoreleasepool {
                             save_ok = fandhe_rust_ios_background_task_fired(0);
                           }
                           BOOL should_finish;
                           @synchronized(task) {
                             should_finish = !finished;
                             finished = YES;
                           }
                           if (should_finish) {
                             [task setTaskCompletedWithSuccess:(save_ok == 0)];
                           }
                         });
                       }];
  g_registered = ok;
  return ok ? 0 : 1;
}

int32_t fandhe_ios_background_task_submit(double earliest_begin_secs) {
#if TARGET_OS_SIMULATOR
  // シミュレータは実機の BGTaskScheduler 発火をサポートしない
  // （実装計画 §5「実挙動の確認」参照）。
  return 3;
#endif
  if (!g_registered) {
    // ハンドラ未登録の予約が発火するとクラッシュするため、未登録時は受理しない
    // （ファイル冒頭コメント参照）。
    return 3;
  }

  // SAFETY: `submit`/`schedule` は `ios_background_task.rs::IosBackgroundTask::ffi`
  // ロックで直列化された Rust ワーカースレッドから呼ばれる契約で、このスレッドには
  // 呼び出し元が autoreleasepool を張らない。`dateWithTimeIntervalSinceNow:` 等が
  // 返す autorelease オブジェクトをこの呼び出し内で drain する
  // （device-bridge-contracts.md「5. FFI 境界」、cursor Low 指摘、#1050）。
  @autoreleasepool {
    // 置き換え契約（`ios_background_task.rs` モジュール doc「世代管理をしない」
    // 参照、codex P1 指摘 #1050 discussion_r4163129008）: 同じ識別子を
    // 取り消さずに再 submit しても OS 側で自動的に置き換わる保証はない
    // （`BGTaskSchedulerErrorCodeTooManyPendingTaskRequests` で失敗し得る）。
    // 既存要求の有無を見ずに毎回先に取り消す（未予約でも安全・べき等、
    // `fandhe_ios_background_task_cancel` と同じ API）ことで、
    // `schedule()` の「置き換えて Ok」契約を保証する。
    [[BGTaskScheduler sharedScheduler]
        cancelTaskRequestWithIdentifier:kFandheBackgroundTaskIdentifier];

    BGAppRefreshTaskRequest *request =
        [[BGAppRefreshTaskRequest alloc] initWithIdentifier:kFandheBackgroundTaskIdentifier];
    if (request == nil) {
      return 4;
    }
    request.earliestBeginDate = [NSDate dateWithTimeIntervalSinceNow:earliest_begin_secs];

    NSError *error = nil;
    BOOL submitted = [[BGTaskScheduler sharedScheduler] submitTaskRequest:request error:&error];
    if (submitted) {
      return 0;
    }
    if (error == nil) {
      return 4;
    }
    if ([error.domain isEqualToString:BGTaskSchedulerErrorDomain]) {
      switch (error.code) {
        case BGTaskSchedulerErrorCodeUnavailable:
          return 1;
        case BGTaskSchedulerErrorCodeNotPermitted:
          return 2;
        default:
          return 4;
      }
    }
    return 4;
  }
}

void fandhe_ios_background_task_cancel(void) {
  // SAFETY: `submit` と同じ理由（直上コメント参照）。`cancelTaskRequestWithIdentifier:`
  // 自体は autorelease オブジェクトを返さないが、`fandhe_ios_background_task_cancel`
  // 呼び出し元の Rust ワーカースレッドに autoreleasepool が無い前提に変わりはないため、
  // 将来の変更でも漏れないよう同じ方針で囲む。
  @autoreleasepool {
    [[BGTaskScheduler sharedScheduler]
        cancelTaskRequestWithIdentifier:kFandheBackgroundTaskIdentifier];
  }
}
