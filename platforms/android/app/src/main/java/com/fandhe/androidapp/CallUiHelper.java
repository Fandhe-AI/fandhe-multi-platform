package com.fandhe.androidapp;

import android.Manifest;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.media.AudioAttributes;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.telecom.DisconnectCause;
import android.telecom.PhoneAccount;
import android.telecom.PhoneAccountHandle;
import android.telecom.TelecomManager;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.annotation.RequiresApi;
import androidx.core.app.NotificationCompat;
import androidx.core.app.NotificationManagerCompat;
import androidx.core.app.Person;
import androidx.core.content.ContextCompat;
import androidx.lifecycle.Lifecycle;

// 通話 UI 統合（Issue #278、親 #87）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_call_ui.rs` が `MainActivity.callUiReport` / `callUiEnd` を JNI で呼び、
// 結果を `nativeOnCallUiEvent` で受ける）。self-managed ConnectionService は OS が着信画面を
// 出さないため、本クラスが着信通知（CallStyle + full-screen intent）を出し、通知の応答・拒否は
// `CallUiActionReceiver` → `CallUiConnectionService.CallConnection` を経て `finish` / `answered` に集まる。
//
// 状態の置き場所: ConnectionService と Receiver は Activity と別に OS から生成されるため、通話の
// 状態は static に置き、main looper の上だけで読み書きする（`ScreenCaptureService.active` と同型）。
// 確定は `finish` / `answered` の 1 か所に集め、状態を確定してから Rust へ届ける（契約表 2）。
//
// 期限（5 秒・30 秒）は Rust のドライバだけが持つ。期限切れ・画面からの取り消しはドライバが先に
// 確定して `end` を呼ぶので、Java は LocalEnded / Unanswered を報告しない。`end` は常に
// `DisconnectCause.LOCAL` で閉じ、`MISSED` は区別しない（trait の `end` に理由引数が無いため）。
//
// ライフサイクル: `onDestroy` で現在の通話を報告なしで閉じる。Rust 側は次の `android_main` の
// `reset_pending_tracking` で揃うため、旧結果が新しい要求の id を消費しない。
//
// 結果コードの SSOT はここで、Rust 側 `android_call_ui.rs` が同じ値で写像する。
// id・発信者名・例外内容はログに出さない。
final class CallUiHelper {
    static final int REPORT_ACCEPTED = 0;
    static final int REPORT_UNSUPPORTED = 1;
    static final int REPORT_FAILED = 2;

    static final int EVENT_RINGING = 0;
    static final int EVENT_ANSWERED = 1;
    static final int EVENT_DECLINED = 2;
    static final int EVENT_REMOTE_ENDED = 3;
    static final int EVENT_FAILED_UNSUPPORTED = 4;
    static final int EVENT_FAILED_PERMISSION = 5;
    static final int EVENT_FAILED_DISALLOWED = 6;
    static final int EVENT_FAILED = 7;

    static final String EXTRA_CALL_ID = "com.fandhe.androidapp.CALL_UI_ID";
    static final String ACTION_ANSWER = "com.fandhe.androidapp.CALL_UI_ANSWER";
    static final String ACTION_DECLINE = "com.fandhe.androidapp.CALL_UI_DECLINE";

    private static final String NOTIFICATION_TAG = "fandhe_call_ui";
    private static final int NOTIFICATION_ID = 1;
    private static final String CHANNEL_ID = "fandhe_call_ui";
    private static final String ACCOUNT_ID = "fandhe_call_ui";

    private static native void nativeOnCallUiEvent(long id, int code);

    // 以下の static は main looper 専用。
    static long currentId = RequestTracker.NONE;
    static CallUiConnectionService.CallConnection connection;
    private static Context appContext;

    private final ComponentActivity activity;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final RequestTracker permissionRequest = new RequestTracker();
    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity の
    // フィールド初期化子から本クラスを生成してここで登録する。
    private final ActivityResultLauncher<String> permissionLauncher;

    CallUiHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
    }

    // ---- 確定（main looper） ----

    /** 終端の通知。id が現在値のときだけ、解放してから Rust へ届ける。 */
    static void finish(long id, int code) {
        if (id != currentId) {
            return;
        }
        close(DisconnectCause.LOCAL);
        nativeOnCallUiEvent(id, code);
    }

    /** 応答。通話は継続するので currentId と connection は残す。 */
    static void answered(long id) {
        if (id != currentId || connection == null) {
            return;
        }
        cancelNotification();
        connection.setActive();
        nativeOnCallUiEvent(id, EVENT_ANSWERED);
    }

    /** 通知・Connection を解放して現在の通話を外す（Rust への報告はしない）。 */
    static void close(int cause) {
        cancelNotification();
        CallUiConnectionService.CallConnection c = connection;
        connection = null;
        currentId = RequestTracker.NONE;
        if (c != null) {
            c.setDisconnected(new DisconnectCause(cause));
            c.destroy();
        }
    }

    static void cancelNotification() {
        if (appContext == null) {
            return;
        }
        try {
            NotificationManagerCompat.from(appContext).cancel(NOTIFICATION_TAG, NOTIFICATION_ID);
        } catch (RuntimeException e) {
            // 解放の失敗は握りつぶす（呼び出し元の確定を止めない）。
        }
    }

    // ---- JNI スレッドから ----

    int report(long id) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return REPORT_UNSUPPORTED;
        }
        PackageManager pm = activity.getPackageManager();
        boolean feature = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                ? pm.hasSystemFeature(PackageManager.FEATURE_TELECOM)
                : pm.hasSystemFeature(PackageManager.FEATURE_CONNECTION_SERVICE);
        if (!feature || activity.getSystemService(TelecomManager.class) == null) {
            return REPORT_UNSUPPORTED;
        }
        main.post(() -> start(id));
        return REPORT_ACCEPTED;
    }

    void end(long id) {
        main.post(() -> {
            if (id != currentId) {
                return;
            }
            permissionRequest.take();
            close(DisconnectCause.LOCAL);
        });
    }

    // MainActivity.onDestroy から（main）。報告なしで閉じる。
    void onDestroy() {
        permissionRequest.take();
        close(DisconnectCause.LOCAL);
    }

    // ---- main looper ----

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private void start(long id) {
        if (destroyed()) {
            nativeOnCallUiEvent(id, EVENT_FAILED);
            return;
        }
        // コンストラクターは Activity.attach 前に呼ばれうるため、ここ（attach 後・main）で取る。
        appContext = activity.getApplicationContext();
        // 前の通話はドライバが先に end 済みのため、残っていれば閉じてから差し替える。
        close(DisconnectCause.LOCAL);
        currentId = id;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                && ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS)
                        != PackageManager.PERMISSION_GRANTED) {
            // 非前景ではダイアログを出せない。
            if (!activity.getLifecycle().getCurrentState().isAtLeast(Lifecycle.State.RESUMED)) {
                finish(id, EVENT_FAILED);
                return;
            }
            permissionRequest.track(id);
            try {
                permissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS);
            } catch (RuntimeException e) {
                permissionRequest.take();
                finish(id, EVENT_FAILED);
            }
            return;
        }
        addCall(id);
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private void onPermissionResult(boolean granted) {
        long id = permissionRequest.take();
        if (id == RequestTracker.NONE || id != currentId) {
            return;
        }
        if (destroyed()) {
            finish(id, EVENT_FAILED);
        } else if (!granted) {
            finish(id, EVENT_FAILED_PERMISSION);
        } else {
            addCall(id);
        }
    }

    private PhoneAccountHandle handle() {
        return new PhoneAccountHandle(
                new ComponentName(activity, CallUiConnectionService.class), ACCOUNT_ID);
    }

    // 通知の可否 → PhoneAccount 登録 → 着信の受付。以降の確定は ConnectionService 側から来る。
    @RequiresApi(Build.VERSION_CODES.O)
    private void addCall(long id) {
        try {
            NotificationManagerCompat nm = NotificationManagerCompat.from(activity);
            if (!nm.areNotificationsEnabled()) {
                finish(id, EVENT_FAILED_PERMISSION);
                return;
            }
            NotificationChannel channel = new NotificationChannel(
                    CHANNEL_ID,
                    activity.getString(R.string.call_ui_channel_name),
                    NotificationManager.IMPORTANCE_HIGH);
            channel.setSound(
                    Settings.System.DEFAULT_RINGTONE_URI,
                    new AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                            .build());
            nm.createNotificationChannel(channel);
            NotificationChannel actual = nm.getNotificationChannel(CHANNEL_ID);
            if (actual != null && actual.getImportance() == NotificationManager.IMPORTANCE_NONE) {
                finish(id, EVENT_FAILED_PERMISSION);
                return;
            }
            TelecomManager tm = activity.getSystemService(TelecomManager.class);
            if (tm == null) {
                finish(id, EVENT_FAILED_UNSUPPORTED);
                return;
            }
            PhoneAccountHandle handle = handle();
            tm.registerPhoneAccount(
                    PhoneAccount.builder(handle, activity.getString(R.string.call_ui_account_label))
                            .setCapabilities(PhoneAccount.CAPABILITY_SELF_MANAGED)
                            .build());
            if (!tm.isIncomingCallPermitted(handle)) {
                finish(id, EVENT_FAILED_DISALLOWED);
                return;
            }
            Bundle extras = new Bundle();
            extras.putLong(EXTRA_CALL_ID, id);
            tm.addNewIncomingCall(handle, extras);
        } catch (SecurityException e) {
            finish(id, EVENT_FAILED_PERMISSION);
        } catch (RuntimeException e) {
            finish(id, EVENT_FAILED);
        }
    }

    /** `Connection.onShowIncomingCallUi` から。通知の投稿に成功したときだけ Ringing を届ける。 */
    @RequiresApi(Build.VERSION_CODES.O)
    static void postIncoming(long id) {
        if (id != currentId || appContext == null) {
            return;
        }
        try {
            Context ctx = appContext;
            Intent open = new Intent(ctx, MainActivity.class)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
            PendingIntent openPi = PendingIntent.getActivity(
                    ctx, 0, open, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
            Person person = new Person.Builder()
                    .setName(ctx.getString(R.string.call_ui_caller_name))
                    .build();
            NotificationCompat.Builder b = new NotificationCompat.Builder(ctx, CHANNEL_ID)
                    .setSmallIcon(android.R.drawable.sym_call_incoming)
                    .setStyle(NotificationCompat.CallStyle.forIncomingCall(
                            person, actionPi(ctx, id, ACTION_DECLINE), actionPi(ctx, id, ACTION_ANSWER)))
                    .setContentIntent(openPi)
                    .setFullScreenIntent(openPi, true)
                    .setCategory(NotificationCompat.CATEGORY_CALL)
                    .setPriority(NotificationCompat.PRIORITY_HIGH)
                    .setOngoing(true);
            NotificationManagerCompat.from(ctx).notify(NOTIFICATION_TAG, NOTIFICATION_ID, b.build());
            nativeOnCallUiEvent(id, EVENT_RINGING);
        } catch (SecurityException e) {
            finish(id, EVENT_FAILED_PERMISSION);
        } catch (RuntimeException e) {
            finish(id, EVENT_FAILED);
        }
    }

    // 明示 Intent + FLAG_IMMUTABLE（他アプリによる書き換え・横取りを防ぐ）。requestCode に id 由来の
    // 値を入れ、世代の違う extras の上書きを避ける。
    private static PendingIntent actionPi(Context ctx, long id, String action) {
        Intent i = new Intent(ctx, CallUiActionReceiver.class).setAction(action);
        i.putExtra(EXTRA_CALL_ID, id);
        int code = (int) ((id & 0x3FFFFFFF) << 1) | (ACTION_ANSWER.equals(action) ? 1 : 0);
        return PendingIntent.getBroadcast(
                ctx, code, i, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }
}
