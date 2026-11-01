package com.fandhe.androidapp;

import android.Manifest;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;

import com.google.android.gms.location.ActivityRecognition;
import com.google.android.gms.location.ActivityRecognitionResult;
import com.google.android.gms.location.DetectedActivity;
import com.google.android.gms.tasks.Task;

import java.util.List;

// 行動認識（歩行・乗車等、Issue #1127、親 #124）の Android 実装ヘルパー。MainActivity から生成・委譲される
// （Rust の `device/android_activity_recognition.rs` が egui スレッドから `MainActivity.activityRecognitionStart` /
// `activityRecognitionRead` / `activityRecognitionStop` を同期 JNI で呼ぶ継続配信の pull 型。`RequestTracker` は使わない）。
// Play services `ActivityRecognitionClient.requestActivityUpdates` の結果を `ActivityRecognitionReceiver`
// （PendingIntent）経由で受け、生の (type, confidence) の組を保持するだけ。種別の写像は Rust `classify_detected` が正。
//
// 結果コードの SSOT はここで、Rust 側 `android_activity_recognition.rs` が同じ値で解釈する。
//   activityRecognitionStart: 0=OK / 1=非対応（API 28 以下・GMS 不可）/ 2=失敗・破棄済み
//   activityRecognitionRead : long[] {status, seq, type0, conf0, type1, conf1, ...}
//     status 0=待機 / 1=値あり / 2=許可拒否 / 3=失敗 / 4=許可ダイアログ待ち / -1=購読が失われた
//
// 設計（OS を正にして状態を減らす）:
// - PendingIntent は 1 つに固定する（requestCode 定数・明示コンポーネント・data なし）。Play services は購読を
//   PendingIntent 単位で持ち、同じ PendingIntent への requestActivityUpdates は前の購読を置き換える。同じ
//   requestCode と Intent（filterEquals）の PendingIntent はプロセス再起動後も同一として照合される。
//   このため世代番号・未解放の記録・解除の再試行は持たない。
//   https://developers.google.com/android/reference/com/google/android/gms/location/ActivityRecognitionClient
//   https://developer.android.com/reference/android/app/PendingIntent （同一性は Intent.filterEquals と requestCode）
// - 状態は main thread だけで読み書きする（Activity のライフサイクル・Task listener（main executor）・
//   BroadcastReceiver.onReceive はすべて main）。Rust ワーカーが JNI で読む値は不変の `Snapshot` を volatile で公開する。
//   start / stop は egui スレッドから main へ post するだけで、main への同期待ちは作らない。
// - 望む状態は `wanted` 1 つ。start / stop / onStart / onStop / 許可結果 / Task 完了は状態を更新して
//   `reconcile()` を呼ぶだけで、reconcile が冪等に購読・解除・許可要求を決める。
//
// 守る範囲: 嘘の値を出さない・二重に購読しない・停止/背景/破棄/失敗で解除を発行する。解除（removeActivityUpdates）の
// は発行した時点で `unreleased`（main thread）を立て、その Task の成功を確認するまで下ろさない。解除 Task が未完了の間は
// 新しい購読も解除の二重発行もせず、失敗（Task 失敗・同期例外）で完了したあとは `reconcile()` が再発行する。
// 失敗が確定している間（`wanted`）は read が Failed を返し、成功と偽らない（完了待ちの間は待機）。開始 Task の失敗でも解除を発行する。再試行のきっかけは start・onStart・onStop・許可結果・Task 完了・Receiver の配送という既存の入口だけで、
// 自前のタイマーや再試行回数は持たない。プロセス再起動後に残った購読も、固定 PendingIntent の次の配送を受けた
// `onResult` が、`wanted` が下りているか前景でなければ解除して値を捨てる。
// 守らない範囲: 入口がまったく来ない間の再試行（プロセス終了時に OS が購読を残しうるが、固定 PendingIntent なので
// 次回起動の reconcile と配送で回収する）、GMS 側の配信間隔と遅延、背景中の受信、過去の履歴、
// start / stop を同一ミリ秒で往復したときの Snapshot の一時的な食い違い。種別・確度はログに出さない。実機確認は #603。
//
// ライフサイクル: `onStop` で解除して最新値を捨て、`onStart` で張り直す（許可ダイアログが起こす onPause とは独立）。
// `onDestroy`（再生成を含む）は解除し、新インスタンスの read は -1 を返す（Rust は失敗扱いで stop する）。
// 期限は Rust driver の `FirstReadingDeadline` だけが持つ。
final class ActivityRecognitionHelper {
    static final int START_OK = 0;
    static final int START_UNSUPPORTED = 1;
    static final int START_FAILED = 2;

    private static final long ST_WAITING = 0;
    private static final long ST_READING = 1;
    private static final long ST_DENIED = 2;
    private static final long ST_FAILED = 3;
    private static final long ST_AWAITING = 4;
    private static final long ST_LOST = -1;

    // ponytail: GMS への希望間隔。実際の配信間隔・遅延は GMS 任せ（守らない範囲）。5 秒以内に最初の 1 件が
    // 届くかは実機未確認の天井。
    private static final long DETECTION_INTERVAL_MS = 1000L;
    // Rust `decode_read` の上限（2 + 2×16）と合わせる。
    private static final int MAX_PAIRS = 16;
    // 固定 PendingIntent の requestCode。
    private static final int REQUEST_CODE = 0x4152; // "AR"

    private static native void nativeOnBackgrounded();
    private static native void nativeOnResumed();

    // Rust から start が 1 度でも呼ばれたか。呼ばれる前は native を呼ばない（feature `device` 無効でも安全）。
    private static volatile boolean nativeReady;
    // start 成功〜stop までの購読意思（Activity 再生成をまたぐ）。新インスタンスの read が LOST かの区別に使う。
    private static volatile boolean subscribed;
    // main thread。現在の Activity のヘルパー。Receiver が状態を参照する。
    private static ActivityRecognitionHelper current;
    // main thread。受理した配送ごとに +1、巻き戻さない。
    private static long seq;
    // main thread。解除が成功と確認できていない。成功でだけ false に戻す。Activity 再生成をまたぐため static。
    private static boolean unreleased;
    // main thread。発行済みで未完了の解除 Task。null でなければ完了待ち（二重発行しない）。
    private static Task<Void> releasing;

    // JNI（egui スレッド）が読む不変スナップショット。
    private static final class Snapshot {
        final long[] values; // {status, seq, type0, conf0, ...}

        Snapshot(long[] values) {
            this.values = values;
        }
    }

    private static final Snapshot WAITING = new Snapshot(new long[] {ST_WAITING, 0});

    private final ComponentActivity activity;
    private final ActivityResultLauncher<String> permissionLauncher;
    private final Handler main = new Handler(Looper.getMainLooper());

    // egui スレッドと main の境界。
    private volatile boolean started;   // start〜stop/destroy
    private volatile boolean destroyed;
    private volatile Snapshot snapshot = WAITING;

    // 以下は main thread だけで読み書きする。
    private boolean wanted;
    private boolean foreground;
    private boolean active;             // requestActivityUpdates を発行済みで、解除を発行していない
    private boolean permissionPending;
    private long failure;               // 0 または ST_DENIED / ST_FAILED
    private long[] latest;              // (type, conf) の組。null=値なし
    private long latestSeq;
    private Task<Void> requestTask;     // 現在の要求。遅れて届いた別の要求の完了を区別する

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity のフィールド初期化子で生成する。
    ActivityRecognitionHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
        current = this;
    }

    // --- egui スレッド ---

    int start() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || !GmsAvailability.isAvailable(activity)) {
            return START_UNSUPPORTED;
        }
        nativeReady = true;
        if (destroyed) return START_FAILED;
        subscribed = true;
        started = true;
        snapshot = WAITING; // 前回の値・失敗を返さない（started を立てるのと同じ同期区間）
        main.post(() -> {
            if (destroyed) return;
            if (!wanted) {
                wanted = true;
                failure = 0;
            }
            reconcile();
        });
        return START_OK;
    }

    long[] read() {
        if (!started) return new long[] {(subscribed || destroyed) ? ST_LOST : ST_WAITING, 0};
        return snapshot.values;
    }

    void stop() {
        subscribed = false;
        started = false;
        snapshot = WAITING;
        main.post(() -> {
            wanted = false;
            failure = 0;
            reconcile();
        });
    }

    // --- main thread ---

    void onStart() {
        foreground = true;
        reconcile();
        if (nativeReady) nativeOnResumed();
    }

    void onStop() {
        foreground = false;
        reconcile();
        if (nativeReady) nativeOnBackgrounded();
    }

    void onDestroy() {
        destroyed = true;
        started = false;
        wanted = false;
        reconcile();
        if (current == this) current = null;
    }

    private void onPermissionResult(boolean granted) {
        permissionPending = false;
        if (wanted && !granted) failure = ST_DENIED;
        reconcile();
    }

    // 冪等。購読・解除・許可要求の判断はここだけで行う。
    private void reconcile() {
        boolean hold = wanted && foreground && !destroyed && failure == 0 && !unreleased;
        if (!hold && active) {
            active = false;
            requestTask = null;
            latest = null;
            release(activity.getApplicationContext());
        } else if (unreleased) {
            release(activity.getApplicationContext()); // 完了待ちなら何もせず、失敗が確定していれば再発行する
        } else if (hold) {
            if (activity.checkSelfPermission(Manifest.permission.ACTIVITY_RECOGNITION)
                    != PackageManager.PERMISSION_GRANTED) {
                if (!permissionPending) {
                    permissionPending = true;
                    try {
                        permissionLauncher.launch(Manifest.permission.ACTIVITY_RECOGNITION);
                    } catch (RuntimeException e) {
                        permissionPending = false;
                        failure = ST_FAILED;
                    }
                }
            } else if (!active) {
                subscribe();
            }
        }
        publish();
    }

    private void subscribe() {
        Context app = activity.getApplicationContext();
        try {
            Task<Void> t = ActivityRecognition.getClient(app)
                    .requestActivityUpdates(DETECTION_INTERVAL_MS, pendingIntent(app));
            active = true;
            requestTask = t;
            t.addOnCompleteListener(ContextCompat.getMainExecutor(app), done -> onRequestDone(done));
        } catch (SecurityException e) {
            failure = ST_DENIED;
            release(app);
        } catch (RuntimeException e) {
            failure = ST_FAILED;
            release(app);
        }
    }

    // 完了時に reconcile を呼ぶので、成功前に wanted が下りていた場合もここで解除に吸収される。
    private void onRequestDone(Task<Void> t) {
        if (t != requestTask) return; // 解除済みの要求の完了
        if (!t.isSuccessful()) {
            Exception e = t.getException();
            active = false;
            requestTask = null;
            latest = null;
            failure = e instanceof SecurityException ? ST_DENIED : ST_FAILED;
            release(activity.getApplicationContext()); // OS 側に購読が残る可能性を否定できない
        }
        reconcile();
    }

    private void publish() {
        long[] v;
        if (failure != 0) v = new long[] {failure, 0};
        else if (unreleased && releasing == null && wanted) v = new long[] {ST_FAILED, 0};
        else if (permissionPending) v = new long[] {ST_AWAITING, 0};
        else if (!wanted || !foreground || latest == null) v = new long[] {ST_WAITING, 0};
        else {
            v = new long[2 + latest.length];
            v[0] = ST_READING;
            v[1] = latestSeq;
            System.arraycopy(latest, 0, v, 2, latest.length);
        }
        snapshot = new Snapshot(v);
    }

    // 配送先（main thread）。購読していない・望まれていない状態の配送は、残った購読とみなして解除し値を捨てる。
    static void onResult(Context context, Intent intent) {
        if (intent == null) return;
        Context app = context.getApplicationContext();
        ActivityRecognitionHelper h = current;
        if (h != null && h.active && !(h.wanted && h.foreground)) {
            h.reconcile(); // 解除を発行して値を捨てる
            return;
        }
        if (h == null || !h.active) {
            release(app);
            return;
        }
        if (!ActivityRecognitionResult.hasResult(intent)) return;
        ActivityRecognitionResult r = ActivityRecognitionResult.extractResult(intent);
        if (r == null) return;
        List<DetectedActivity> list = r.getProbableActivities();
        int n = Math.min(list.size(), MAX_PAIRS);
        long[] pairs = new long[n * 2];
        for (int i = 0; i < n; i++) {
            pairs[2 * i] = list.get(i).getType();
            pairs[2 * i + 1] = list.get(i).getConfidence();
        }
        h.latest = pairs;
        h.latestSeq = ++seq;
        h.publish();
    }

    // 固定 PendingIntent で解除する。発行時に unreleased を立て、Task の成功でだけ下ろして pi.cancel() する
    // （unreleased の間は新しい購読をしないので、cancel が新しい購読を壊さない）。完了待ちの間は何もしない。
    // 成功時は reconcile で購読の再開を試みる。失敗時は unreleased を残して reconcile を呼ばない
    // （自前の再試行ループを作らない。次の入口が再発行する）。main thread で呼ぶ。
    private static void release(Context app) {
        if (releasing != null) return;
        unreleased = true;
        PendingIntent pi = pendingIntent(app);
        try {
            Task<Void> task = ActivityRecognition.getClient(app).removeActivityUpdates(pi);
            releasing = task;
            task.addOnCompleteListener(ContextCompat.getMainExecutor(app), t -> {
                releasing = null;
                if (t.isSuccessful()) {
                    unreleased = false;
                    pi.cancel();
                }
                ActivityRecognitionHelper h = current;
                if (h == null) return;
                if (unreleased) h.publish(); else h.reconcile();
            });
        } catch (RuntimeException e) {
            releasing = null;
        }
    }

    // GMS が extras を書き込むため API 31 以上は FLAG_MUTABLE。明示コンポーネント宛てなので他アプリには横取りされない。
    // requestCode・Intent を固定し、常に同一の PendingIntent として照合させる。
    private static PendingIntent pendingIntent(Context app) {
        Intent i = new Intent(app, ActivityRecognitionReceiver.class);
        int flags = PendingIntent.FLAG_UPDATE_CURRENT
                | (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ? PendingIntent.FLAG_MUTABLE : 0);
        return PendingIntent.getBroadcast(app, REQUEST_CODE, i, flags);
    }
}
