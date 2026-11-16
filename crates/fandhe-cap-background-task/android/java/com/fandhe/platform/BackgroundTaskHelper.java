package com.fandhe.platform;

import android.content.Context;

import androidx.work.ExistingWorkPolicy;
import androidx.work.OneTimeWorkRequest;
import androidx.work.Operation;
import androidx.work.WorkManager;

import java.util.concurrent.ExecutionException;

// バックグラウンド処理（Issue #1046）の Android 実装ヘルパー。Rust の `fandhe-cap-background-task/src/android.rs` が
// static の `schedule(Context)` / `cancel(Context)` を JNI で同期に呼ぶ（状態を持たないため provider は作らない）。
// 予約の実体（記録の書き込み）は `BackgroundTaskWorker` が `getFilesDir()` 直下へ直接行う
// （Rust 側 JNI 呼び出しは `schedule` / `cancel` の橋渡しのみ。記録を書く主体は `android.rs` のモジュール doc 参照）。
final class BackgroundTaskHelper {
    // `WorkManager` の一意名。
    private static final String WORK_NAME = "fandhe.background-task";

    private BackgroundTaskHelper() {}

    // `schedule` / `cancel` の結果（`Operation.getResult()`）を、確定するまで待つ。待ち上限を設けると、上限を過ぎた後に
    // 操作が確定して戻り値と実際の予約が食い違い、取り消しで巻き戻そうとすると置き換え前の予約まで失う。
    // 呼び出し元（Rust 側 `AndroidBackgroundTask`）は UI スレッド外の作業スレッドから呼ぶため、無期限に待っても
    // 画面は固まらない。割込みでも中断せず確定まで待ち、割込み状態は最後に戻す。戻り値: 0 = 成功、1 = 失敗。
    // ponytail: 背景アップロード（MainActivity.awaitOperation を使う）が機能 crate へ移るとき 1 つに寄せる。
    private static int awaitOperation(Operation operation) {
        boolean interrupted = false;
        try {
            while (true) {
                try {
                    operation.getResult().get();
                    return 0;
                } catch (InterruptedException e) {
                    interrupted = true;
                } catch (ExecutionException | RuntimeException e) {
                    return 1;
                }
            }
        } finally {
            if (interrupted) {
                Thread.currentThread().interrupt();
            }
        }
    }

    // 処理 1 件を一意名で予約する。二重予約は常に置き換える（`ExistingWorkPolicy.REPLACE`、契約）。
    // 戻り値: 0 = 成功、それ以外 = 失敗。
    static int schedule(Context activity) {
        try {
            OneTimeWorkRequest request = new OneTimeWorkRequest.Builder(BackgroundTaskWorker.class).build();
            return awaitOperation(
                    WorkManager.getInstance(activity.getApplicationContext())
                            .enqueueUniqueWork(WORK_NAME, ExistingWorkPolicy.REPLACE, request));
        } catch (RuntimeException e) {
            return 1;
        }
    }

    // 予約を取り消す。未予約でも `cancelUniqueWork` 自体は成功する（べき等。Rust 側はこの戻り値 0 をそのまま `Ok(())` として
    // 扱う）。戻り値: 0 = 成功、それ以外 = 失敗。
    static int cancel(Context activity) {
        try {
            return awaitOperation(
                    WorkManager.getInstance(activity.getApplicationContext()).cancelUniqueWork(WORK_NAME));
        } catch (RuntimeException e) {
            return 1;
        }
    }
}
