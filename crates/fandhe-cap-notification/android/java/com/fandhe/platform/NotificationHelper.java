package com.fandhe.platform;

import android.Manifest;
import android.app.Activity;
import android.content.pm.PackageManager;
import android.os.Build;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.app.NotificationChannelCompat;
import androidx.core.app.NotificationCompat;
import androidx.core.app.NotificationManagerCompat;
import androidx.core.content.ContextCompat;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// ローカル通知（Issue #1006、Phase 11 型サンプル「通知」の Android 経路）の Android 実装ヘルパー。Activity ごとに 1 つ、
// `NotificationLifecycle`（`FeatureLifecycleProvider` 継承）が `attach` で生成して `HELPERS` に持つ。
// Rust の `fandhe-cap-notification`（`src/android.rs`）が static の `requestNotification`（第 1 引数は Activity）を JNI で呼び、
// 結果は `nativeOnNotificationResult` で非同期に返す（Rust 側 `driver::bridge`（`RequestSlot` の facade）と pull 型で橋渡しする）。
// helper が無い（生成前・破棄後）要求は成功にも待ちにもせず、内部エラーのコードで確定する。
//
// code の意味（本ファイルが SSOT、Rust 側 `driver::outcome_from_code` の doc と対）:
//   0=送信した（`NotificationManagerCompat.notify` が例外無しで完了）
//   1=キャンセル（Android では出ない。`ActivityResultContracts.RequestPermission` は「拒否」と「ダイアログを閉じただけ」を区別できない）
//   2=権限拒否（実行時許可の拒否・通知オフ・チャンネルブロックを区別せず畳む）
//   3=内部エラー（権限確認自体の失敗）
//   4=内部エラー（`notify()` 呼び出し自体の失敗）
//
// 許可ランチャーは `attach`（`Activity.onCreate` の中、STARTED より前）で登録する。登録キーは従来の文字列のまま。
// 権限ダイアログ待ちの要求は Activity の破棄で捨てる（復元しない。Rust 側 `android_main` の `reset_pending_tracking` と対）。
// ダイアログ自体が Activity を pause させるため、許可コールバックでは前面かどうかを見ない（通知の投稿は背景でも正当）。
// 守る範囲: 1 プロセス内の要求の照合（旧要求の遅延到着は捨てる）・嘘の成功の禁止。
// 守らない範囲: 通知の表示の確認（`Shown` は返さない）・通知タップへの応答（contentIntent を付けない）。
// タイトル・本文は表示にだけ使い、ログへ出さない（security.md A03/A09）。
final class NotificationHelper {
    private static final int RESULT_SUBMITTED = 0;
    // Android では発生しない。Rust 側 `outcome_from_code` の写像表との対応を示すためだけに宣言する。
    @SuppressWarnings("unused")
    private static final int RESULT_CANCELLED = 1;
    private static final int RESULT_PERMISSION_DENIED = 2;
    private static final int RESULT_AUTH_ERROR = 3;
    private static final int RESULT_POST_ERROR = 4;

    // 通知チャンネル ID。
    private static final String CHANNEL_ID = "fandhe_default";

    private static native void nativeOnNotificationResult(long requestId, int code);

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, NotificationHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final ComponentActivity activity;
    private final ActivityResultLauncher<String> permissionLauncher;

    // 以下は UI スレッドだけで読み書きする。
    private final RequestTracker request = new RequestTracker();
    // 権限ダイアログ表示中に許可コールバックが読むタイトル・本文。
    private String pendingTitle;
    private String pendingBody;
    private boolean foreground;

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、`NotificationLifecycle.onHostCreated`
    // （`Activity.onCreate` の中）から `attach` で生成する。
    private NotificationHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.getActivityResultRegistry().register(
                "fandhe:MainActivity:1", activity, new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
    }

    // Activity の生成時（`NotificationLifecycle` から、UI スレッド）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new NotificationHelper(activity));
    }

    // Activity の破棄時（同上）。保留中の要求は復元せず捨てる。
    static void detach(Activity activity) {
        NotificationHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.request.take();
            helper.pendingTitle = null;
            helper.pendingBody = null;
        }
    }

    // `NotificationLifecycle` から（UI スレッド）。前面にいる間だけ権限ダイアログを出す。
    static void resumed(Activity activity) {
        NotificationHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.foreground = true;
        }
    }

    static void paused(Activity activity) {
        NotificationHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.foreground = false;
        }
    }

    // Rust から呼ぶ入口。即時 return で、結果は `nativeOnNotificationResult` が非同期に返す。
    // helper が無い要求は内部エラーで確定する（結果が来ないまま待たせない）。
    static void requestNotification(Activity activity, long requestId, String title, String body) {
        NotificationHelper helper = HELPERS.get(activity);
        if (helper == null) {
            nativeOnNotificationResult(requestId, RESULT_AUTH_ERROR);
            return;
        }
        activity.runOnUiThread(() -> helper.begin(requestId, title, body));
    }

    private void begin(long requestId, String title, String body) {
        if (Build.VERSION.SDK_INT < 33
                || ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS)
                        == PackageManager.PERMISSION_GRANTED) {
            // API 33 未満は実行時許可自体が存在しないため要求しない（device-bridge-contracts.md「対応表・権限」節）。
            post(requestId, title, body);
            return;
        }
        if (activity.isFinishing() || activity.isDestroyed() || !foreground) {
            // 非前景でダイアログを出さない。
            nativeOnNotificationResult(requestId, RESULT_AUTH_ERROR);
            return;
        }
        request.track(requestId);
        pendingTitle = title;
        pendingBody = body;
        try {
            permissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS);
        } catch (RuntimeException e) {
            // `launch()` 自体が例外を送出した場合。コールバックは二度と来ないため、ここで直接失敗を通知する。
            request.take();
            pendingTitle = null;
            pendingBody = null;
            nativeOnNotificationResult(requestId, RESULT_AUTH_ERROR);
        }
    }

    // 通知の実行時許可要求の結果（UI スレッド）。許可が下りたら `post` を呼ぶ。
    private void onPermissionResult(boolean granted) {
        long id = request.take();
        if (id == RequestTracker.NONE) {
            // 既に破棄済み（Activity 再生成等）。
            return;
        }
        String title = pendingTitle;
        String body = pendingBody;
        pendingTitle = null;
        pendingBody = null;
        if (!granted) {
            nativeOnNotificationResult(id, RESULT_PERMISSION_DENIED);
            return;
        }
        post(id, title, body);
    }

    // 権限確認後（`begin` / `onPermissionResult` のいずれか）に呼ぶ。常に UI スレッドから呼ばれる。
    private void post(long requestId, String title, String body) {
        NotificationManagerCompat manager = NotificationManagerCompat.from(activity);
        NotificationChannelCompat channel = new NotificationChannelCompat.Builder(
                        CHANNEL_ID, NotificationManagerCompat.IMPORTANCE_DEFAULT)
                .setName(activity.getString(R.string.notification_channel_name))
                .build();
        try {
            // `areNotificationsEnabled`/`createNotificationChannel`/`getNotificationChannelCompat` は通知サービスとの
            // IPC を伴い、サービス側の異常で `RuntimeException` を送出し得る。try の外に置くと UI スレッドの未捕捉例外で
            // アプリが終了し、`nativeOnNotificationResult` へ失敗を返せなくなる。
            if (!manager.areNotificationsEnabled()) {
                // API 33 未満で利用者が設定から通知を切った場合を含む（嘘の成功を出さない）。
                nativeOnNotificationResult(requestId, RESULT_PERMISSION_DENIED);
                return;
            }
            manager.createNotificationChannel(channel);
            NotificationChannelCompat created = manager.getNotificationChannelCompat(CHANNEL_ID);
            if (created != null && created.getImportance() == NotificationManagerCompat.IMPORTANCE_NONE) {
                // チャンネル単位でブロックされている（設定アプリからチャンネルを個別にオフにした場合）。
                nativeOnNotificationResult(requestId, RESULT_PERMISSION_DENIED);
                return;
            }
        } catch (RuntimeException e) {
            nativeOnNotificationResult(requestId, RESULT_POST_ERROR);
            return;
        }
        NotificationCompat.Builder builder = new NotificationCompat.Builder(activity, CHANNEL_ID)
                .setSmallIcon(android.R.drawable.sym_def_app_icon)
                .setContentTitle(title)
                .setContentText(body)
                .setAutoCancel(true);
        try {
            manager.notify((int) requestId, builder.build());
            nativeOnNotificationResult(requestId, RESULT_SUBMITTED);
        } catch (SecurityException e) {
            nativeOnNotificationResult(requestId, RESULT_PERMISSION_DENIED);
        } catch (RuntimeException e) {
            nativeOnNotificationResult(requestId, RESULT_POST_ERROR);
        }
    }
}
