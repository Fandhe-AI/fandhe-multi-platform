package com.fandhe.androidapp;

import android.Manifest;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.pm.PackageManager;
import android.os.Build;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.app.NotificationCompat;
import androidx.core.app.NotificationManagerCompat;
import androidx.core.content.ContextCompat;
import androidx.lifecycle.Lifecycle;

// アプリアイコンのバッジ画面（Issue #1362、親 #1141）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される（Rust の `device/android_app_badge.rs` が `MainActivity.requestAppBadge` を
// JNI で呼び、結果を `nativeOnAppBadgeResult` で受ける）。
//
// Android にバッジ専用 API は無いため、`setNumber(n)` を付けた無音の通知を固定 tag/ID で 1 件出す。
// 0 は許可を問わず `cancel` で消す。API 26 未満は OS がドットを出さないので UNSUPPORTED。
//
// ライフサイクル: `onDestroy` 連携は持たない。`request` と `pendingValue` はインスタンス
// フィールドなので、再生成後の新インスタンスは NONE から始まり旧ダイアログの結果を拾わない。
// 旧インスタンスの Runnable は入口の破棄判定で FAILED を返し、その結果も Rust 側の
// `reset_pending_tracking()` と id 照合で捨てられる。
//
// 結果コードの SSOT はここで、Rust 側 `android_app_badge.rs::decode` が同じ値で写像する。
// 値・例外メッセージはログに出さない。全処理は UI スレッドで直列化する。
final class AppBadgeHelper {
    static final int RESULT_SUBMITTED = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_PERMISSION_DENIED = 2;
    static final int RESULT_FAILED = 3;

    private static final String TAG = "fandhe_app_badge";
    private static final int NOTIFICATION_ID = 1;
    private static final String CHANNEL_ID = "fandhe_app_badge";

    private static native void nativeOnAppBadgeResult(long requestId, int value, int code);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private int pendingValue; // 許可ダイアログ中の値（UI スレッド専用）。
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity の
    // フィールド初期化子から本クラスを生成してここで登録する。
    private final ActivityResultLauncher<String> permissionLauncher;

    AppBadgeHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
    }

    // JNI スレッドから呼ばれる。
    void set(long id, int value) {
        activity.runOnUiThread(() -> start(id, value));
    }

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    private void finish(long id, int value, int code) {
        nativeOnAppBadgeResult(id, value, code);
    }

    private void start(long id, int value) {
        if (destroyed()) {
            finish(id, value, RESULT_FAILED);
            return;
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            finish(id, value, RESULT_UNSUPPORTED);
            return;
        }
        if (value == 0) {
            try {
                NotificationManagerCompat.from(activity).cancel(TAG, NOTIFICATION_ID);
                finish(id, value, RESULT_SUBMITTED);
            } catch (RuntimeException e) {
                finish(id, value, RESULT_FAILED);
            }
            return;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                && ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS)
                        != PackageManager.PERMISSION_GRANTED) {
            // 非前景ではダイアログを出せない。
            if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
                finish(id, value, RESULT_FAILED);
                return;
            }
            request.track(id);
            pendingValue = value;
            try {
                permissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS);
            } catch (RuntimeException e) {
                request.take();
                finish(id, value, RESULT_FAILED);
            }
            return;
        }
        post(id, value);
    }

    private void onPermissionResult(boolean granted) {
        long id = request.take();
        if (id == RequestTracker.NONE) {
            return;
        }
        int value = pendingValue;
        if (destroyed()) {
            finish(id, value, RESULT_FAILED);
        } else if (!granted) {
            finish(id, value, RESULT_PERMISSION_DENIED);
        } else {
            post(id, value);
        }
    }

    // API 26+ でのみ呼ばれる（start の入口で弾く）。IPC の例外で UI スレッドを落とさない。
    @androidx.annotation.RequiresApi(Build.VERSION_CODES.O)
    private void post(long id, int value) {
        try {
            NotificationManagerCompat nm = NotificationManagerCompat.from(activity);
            if (!nm.areNotificationsEnabled()) {
                finish(id, value, RESULT_PERMISSION_DENIED);
                return;
            }
            NotificationChannel channel = new NotificationChannel(
                    CHANNEL_ID,
                    activity.getString(R.string.app_badge_channel_name),
                    NotificationManager.IMPORTANCE_LOW);
            channel.setShowBadge(true);
            nm.createNotificationChannel(channel);
            NotificationChannel actual = nm.getNotificationChannel(CHANNEL_ID);
            if (actual != null && actual.getImportance() == NotificationManager.IMPORTANCE_NONE) {
                finish(id, value, RESULT_PERMISSION_DENIED);
                return;
            }
            // contentIntent は付けない（exported な入口を増やさない）。
            NotificationCompat.Builder b = new NotificationCompat.Builder(activity, CHANNEL_ID)
                    .setSmallIcon(android.R.drawable.sym_def_app_icon)
                    .setContentTitle(activity.getString(R.string.app_name))
                    .setContentText(activity.getString(R.string.app_badge_notification_text))
                    .setNumber(value)
                    .setSilent(true)
                    .setOnlyAlertOnce(true)
                    .setOngoing(false);
            nm.notify(TAG, NOTIFICATION_ID, b.build());
            finish(id, value, RESULT_SUBMITTED);
        } catch (SecurityException e) {
            finish(id, value, RESULT_PERMISSION_DENIED);
        } catch (RuntimeException e) {
            finish(id, value, RESULT_FAILED);
        }
    }
}
