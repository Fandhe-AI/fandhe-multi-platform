package com.fandhe.platform;

import android.Manifest;
import android.app.Activity;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.pm.PackageManager;
import android.os.Build;
import android.service.notification.StatusBarNotification;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.app.NotificationCompat;
import androidx.core.app.NotificationManagerCompat;
import androidx.core.content.ContextCompat;
import androidx.lifecycle.Lifecycle;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// ライブアクティビティ画面（Issue #296、親 #88、#1187）の Android 実装ヘルパー。利用者の `MainActivity` には
// 何も書かせない。Rust の `src/android.rs` が static の `submit(Activity, id, op, value)` /
// `dismissAll(Activity)` を JNI で呼び、結果を `nativeOnLiveActivityResult` で受ける。Activity への参加は
// `LiveActivityLifecycle`（非公開 ContentProvider、`FeatureLifecycleProvider` 継承）が行い、生成時に `attach`、
// 破棄時に `detach` を呼ぶ。Activity ごとの helper は `HELPERS` に持つ（破棄時に必ず外す）。
// 表に無い Activity への `submit` は RESULT_FAILED を返す（結果が来ないまま待たせない）。
//
// 数値 1 つを進行中通知（`setOngoing(true)` + `setProgress`）として固定 tag/ID で 1 件出す。
// Android 16 の Live Updates（ProgressStyle）は使わない。API 24〜25 は channel が無いので UNSUPPORTED。
// 終了は許可を問わず cancel で冪等。更新は通知が無ければ DISMISSED（再投稿しない）。
// 許可ダイアログは開始のときだけ出す。
//
// ライフサイクル: 破棄時は表から外すだけ。`request` と `pendingValue` はインスタンスフィールドなので
// 再生成後の新インスタンスは NONE から始まり旧ダイアログの結果を拾わない。旧インスタンスの Runnable は
// 入口の破棄判定で FAILED を返し、Rust 側の `reset_pending_tracking()`（`dismissAll` を含む）と id 照合で
// 捨てられる。
//
// 結果コードの SSOT はここで、Rust 側 `android.rs::decode` が同じ値で写像する。
// 値・例外メッセージはログに出さない。全処理は UI スレッドで直列化する。
final class LiveActivityHelper {
    static final int OP_START = 0;
    static final int OP_UPDATE = 1;
    static final int OP_END = 2;

    static final int RESULT_STARTED = 0;
    static final int RESULT_UPDATED = 1;
    static final int RESULT_ENDED = 2;
    static final int RESULT_DISMISSED = 3;
    static final int RESULT_UNSUPPORTED = 4;
    static final int RESULT_PERMISSION_DENIED = 5;
    static final int RESULT_FAILED = 6;

    private static final String TAG = "fandhe_live_activity";
    private static final int NOTIFICATION_ID = 1;
    private static final String CHANNEL_ID = "fandhe_live_activity";
    private static final int MAX_VALUE = 9999;

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, LiveActivityHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private static final String KEY_PERMISSION = "fandhe.live-activity.permission";

    private static native void nativeOnLiveActivityResult(long requestId, int op, int code);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private int pendingValue; // 許可ダイアログ中の値（UI スレッド専用）。
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、`attach`（`Activity.onCreate` 内）で
    // 本クラスを生成してここで登録する。キーは明示する（連番だと生成順で結果が別のランチャーへ届く）。
    private final ActivityResultLauncher<String> permissionLauncher;

    private LiveActivityHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.getActivityResultRegistry().register(KEY_PERMISSION, activity,
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
    }

    // Activity の生成時（`LiveActivityLifecycle` から）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new LiveActivityHelper(activity));
    }

    // Activity の破棄時（`LiveActivityLifecycle` から）。
    static void detach(Activity activity) {
        HELPERS.remove(activity);
    }

    // JNI スレッド（Rust のワーカー）から呼ばれる入口。helper が無い（生成前・破棄後）なら失敗で確定する。
    static void submit(Activity activity, long id, int op, int value) {
        LiveActivityHelper helper = HELPERS.get(activity);
        if (helper == null) {
            nativeOnLiveActivityResult(id, op, RESULT_FAILED);
            return;
        }
        activity.runOnUiThread(() -> helper.start(id, op, value));
    }

    // JNI スレッドから呼ばれる。結果は返さない。再生成直後の新インスタンスから呼ばれ、
    // main looper の FIFO なので後に来る新しい要求より先に走る。helper を介さない
    // （`android_main` が `attach` より先に走っても片付けが落ちないように）。
    static void dismissAll(Activity activity) {
        activity.runOnUiThread(() -> {
            try {
                NotificationManagerCompat.from(activity).cancel(TAG, NOTIFICATION_ID);
            } catch (RuntimeException e) {
                // 冪等な片付けなので握りつぶす。
            }
        });
    }

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    private void finish(long id, int op, int code) {
        nativeOnLiveActivityResult(id, op, code);
    }

    private boolean permissionMissing() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                && ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS)
                        != PackageManager.PERMISSION_GRANTED;
    }

    private void start(long id, int op, int value) {
        if (destroyed()) {
            finish(id, op, RESULT_FAILED);
            return;
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            finish(id, op, RESULT_UNSUPPORTED);
            return;
        }
        if (op == OP_END) {
            try {
                NotificationManagerCompat.from(activity).cancel(TAG, NOTIFICATION_ID);
                finish(id, op, RESULT_ENDED);
            } catch (RuntimeException e) {
                finish(id, op, RESULT_FAILED);
            }
            return;
        }
        if (op != OP_START && op != OP_UPDATE) {
            finish(id, op, RESULT_FAILED);
            return;
        }
        if (value < 0 || value > MAX_VALUE) {
            finish(id, op, RESULT_FAILED);
            return;
        }
        if (permissionMissing()) {
            if (op == OP_UPDATE) {
                // 更新ではダイアログを出さない。
                finish(id, op, RESULT_PERMISSION_DENIED);
                return;
            }
            // 非前景ではダイアログを出せない。
            if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
                finish(id, op, RESULT_FAILED);
                return;
            }
            request.track(id);
            pendingValue = value;
            try {
                permissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS);
            } catch (RuntimeException e) {
                request.take();
                finish(id, op, RESULT_FAILED);
            }
            return;
        }
        post(id, op, value);
    }

    private void onPermissionResult(boolean granted) {
        long id = request.take();
        if (id == RequestTracker.NONE) {
            return;
        }
        if (destroyed()) {
            finish(id, OP_START, RESULT_FAILED);
        } else if (!granted) {
            finish(id, OP_START, RESULT_PERMISSION_DENIED);
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            post(id, OP_START, pendingValue);
        } else {
            finish(id, OP_START, RESULT_UNSUPPORTED);
        }
    }

    private boolean isShowing(NotificationManager nm) {
        for (StatusBarNotification n : nm.getActiveNotifications()) {
            if (n.getId() == NOTIFICATION_ID && TAG.equals(n.getTag())) {
                return true;
            }
        }
        return false;
    }

    // API 26+ でのみ呼ばれる（start の入口で弾く）。IPC の例外で UI スレッドを落とさない。
    @androidx.annotation.RequiresApi(Build.VERSION_CODES.O)
    private void post(long id, int op, int value) {
        try {
            NotificationManagerCompat nm = NotificationManagerCompat.from(activity);
            if (!nm.areNotificationsEnabled()) {
                finish(id, op, RESULT_PERMISSION_DENIED);
                return;
            }
            NotificationChannel channel = new NotificationChannel(
                    CHANNEL_ID,
                    activity.getString(R.string.live_activity_channel_name),
                    NotificationManager.IMPORTANCE_LOW);
            channel.setShowBadge(false);
            nm.createNotificationChannel(channel);
            NotificationChannel actual = nm.getNotificationChannel(CHANNEL_ID);
            if (actual != null && actual.getImportance() == NotificationManager.IMPORTANCE_NONE) {
                finish(id, op, RESULT_PERMISSION_DENIED);
                return;
            }
            if (op == OP_UPDATE) {
                // 利用者が閉じた通知を再投稿しない（Android 14+ で閉じられる）。
                NotificationManager sys = activity.getSystemService(NotificationManager.class);
                if (sys == null || !isShowing(sys)) {
                    finish(id, op, RESULT_DISMISSED);
                    return;
                }
            }
            // contentIntent は付けない（exported な入口を増やさない）。値は秘密でない数値なので
            // ロック画面にも出す。
            NotificationCompat.Builder b = new NotificationCompat.Builder(activity, CHANNEL_ID)
                    .setSmallIcon(android.R.drawable.sym_def_app_icon)
                    .setContentTitle(activity.getString(R.string.live_activity_notification_title))
                    .setContentText(activity.getString(R.string.live_activity_notification_prefix) + value)
                    .setProgress(MAX_VALUE, value, false)
                    .setOngoing(true)
                    .setOnlyAlertOnce(true)
                    .setSilent(true)
                    .setVisibility(NotificationCompat.VISIBILITY_PUBLIC);
            nm.notify(TAG, NOTIFICATION_ID, b.build());
            finish(id, op, op == OP_START ? RESULT_STARTED : RESULT_UPDATED);
        } catch (SecurityException e) {
            finish(id, op, RESULT_PERMISSION_DENIED);
        } catch (RuntimeException e) {
            finish(id, op, RESULT_FAILED);
        }
    }
}
