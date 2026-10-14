// Phase 11 の拡充ループ（親 #1042）型サンプル「バックグラウンド処理」: macOS 実装
// （Issue #1043）。`NSBackgroundActivityScheduler` は macOS 専用 API（iOS には存在
// しない）のため、iOS/macOS 共用の `apple/*.m` ではなくここ（`macos/` 配下）に置く
// （`apple/fandhe_apple_location.m` 等との違い）。
//
// `crates/fandhe-egui-proxy/src/device/macos_background_task.rs` から C ABI 経由で
// 呼ばれる。`build.rs` の `build_macos()` が feature `device` 有効時にのみ
// コンパイルし、Foundation をリンクする。
//
// 契約:
//   fandhe_macos_background_task_schedule(uint64_t generation, double interval,
//     double tolerance) -> int32_t
//     0 = 予約を受理（`NSBackgroundActivityScheduler` を生成して
//     `scheduleWithBlock:` まで完了した）。1 = 生成に失敗（呼び出し元は Rust 側の
//     世代を元に戻す、`macos_background_task.rs::schedule` 参照）。新しい
//     scheduler の生成に成功してから既存の予約を `invalidate` して置き換える
//     （二重予約は置き換え契約）。生成失敗時は既存の `g_scheduler` に触れないため、
//     既存予約は失われない。identifier は `generation` ごとに一意にする（固定の
//     identifier を新旧で共有すると、新しい scheduler を登録した直後に旧
//     scheduler を `invalidate` したとき、システム側が identifier を鍵に登録解除し
//     新しい予約まで巻き込んで解除しうるため。Cursor Bugbot 指摘対応）。
//   fandhe_macos_background_task_cancel(void) -> void
//     既存の予約があれば `invalidate` して nil にする。未予約でも安全（べき等）。
//   fandhe_rust_background_task_fired(uint64_t generation) -> Rust 側
//     （`macos_background_task.rs`）が公開する `extern "C"` 関数。block が実際に
//     OS から呼ばれた瞬間に渡した `generation` とともに呼ぶ。
//
// スレッド契約: `scheduleWithBlock:` に渡す block は `NSBackgroundActivityScheduler`
// の private queue から届く（main queue ではない）。block の内側は
// `@autoreleasepool` で囲む（`device-bridge-contracts.md`「5. FFI 境界」）。
// `fandhe_rust_background_task_fired` の呼び出し後、completion は必ず 1 回だけ呼ぶ
// （二重呼び出し・無呼び出しのいずれも禁止）。
//
// `shouldDefer` は見ない: 本画面の処理は保存先へ時刻を書くだけで一瞬のため、
// 延期してもしなくても体感差が無い（`ponytail:` 延期を使う重い処理が要件になったら
// `shouldDefer` 分岐を追加する）。
//
// Rust 側が `schedule`/`cancel` の呼び出しを `Mutex` で直列化する契約
// （`macos_background_task.rs::MacBackgroundTask::ffi` 参照）のため、この .m 側は
// 呼び出し間の排他を自前で持たない。`g_scheduler` への書き込みは
// `fandhe_macos_background_task_schedule`/`_cancel`（呼び出し元が直列化済み）と
// block 内の読み取りのみで、block 自体は `g_scheduler` を書き換えない。
//
// アプリ終了時: プロセスが終わると `g_scheduler` も含め全リソースが消えるため、
// 明示的な解放処理は不要（`macos_background_task.rs` モジュール doc 参照）。

#import <Foundation/Foundation.h>

static NSBackgroundActivityScheduler *g_scheduler = nil;

extern void fandhe_rust_background_task_fired(uint64_t generation);

int32_t fandhe_macos_background_task_schedule(uint64_t generation, double interval,
                                               double tolerance) {
  // 新しい scheduler の生成に成功してから既存の予約を invalidate する
  // （生成失敗時に既存予約を失わないため。device-bridge-contracts.md
  // 「9. 失敗時のリソース解放・共有状態復元」参照）。identifier に generation を
  // 含めて新旧で一意にする（固定 identifier の共有による invalidate の巻き込みを
  // 避けるため、ファイル冒頭コメント参照）。
  NSString *identifier =
      [NSString stringWithFormat:@"com.fandhe.egui-proxy.background-task.%llu",
                                  (unsigned long long)generation];
  NSBackgroundActivityScheduler *scheduler =
      [[NSBackgroundActivityScheduler alloc] initWithIdentifier:identifier];
  if (scheduler == nil) {
    return 1;
  }
  scheduler.repeats = NO;
  scheduler.interval = interval;
  scheduler.tolerance = tolerance;
  scheduler.qualityOfService = NSQualityOfServiceUserInitiated;

  [scheduler scheduleWithBlock:^(NSBackgroundActivityCompletionHandler completedHandler) {
    // SAFETY（契約節参照）: この block は private queue から届く。
    // `fandhe_rust_background_task_fired` へ渡すのは `generation`（u64）のみで
    // ポインタは渡さない。
    @autoreleasepool {
      fandhe_rust_background_task_fired(generation);
    }
    completedHandler(NSBackgroundActivityResultFinished);
  }];

  if (g_scheduler != nil) {
    [g_scheduler invalidate];
  }
  g_scheduler = scheduler;
  return 0;
}

void fandhe_macos_background_task_cancel(void) {
  if (g_scheduler != nil) {
    [g_scheduler invalidate];
    g_scheduler = nil;
  }
}
