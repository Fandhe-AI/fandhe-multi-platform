package com.fandhe.androidapp;

import android.Manifest;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import com.google.android.gms.location.ActivityRecognition;
import com.google.android.gms.location.ActivityRecognitionResult;
import com.google.android.gms.location.DetectedActivity;

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
// スレッド: start/read/stop は egui スレッド、Receiver・Task コールバック・許可結果・ライフサイクルは main looper。
// 状態はすべて static `LOCK` の内側で読み書きする。GMS 呼び出しは即戻るため LOCK 内で発行し、
// native 通知は LOCK の外で呼ぶ。main への同期待ちは作らない。
//
// 世代: 購読のたびに `gen` を進め、PendingIntent の requestCode と extra を世代ごとに分ける。
// 遅れて届いた旧世代の完了・配送は照合して捨て、その世代の購読だけを remove する（新しい購読を消さない）。
// プロセス死後に GMS へ残った孤児購読も、届いた配送が現世代と違うことで remove・cancel して回収する。
//
// ライフサイクル: `onStop` で購読を解除して最新値を捨て、`onStart` で新世代として張り直す
// （許可ダイアログが起こす onPause とは独立）。背景中に届いた許可結果ではここでは購読せず onStart に任せる。
// `onDestroy`（再生成を含む）は購読を解除し、新インスタンスの read は -1 を返す（Rust は失敗扱いで stop する）。
// 期限は Rust driver の `FirstReadingDeadline` だけが持つ。
//
// 守る範囲: 嘘の値を出さない・二重に購読しない・停止/背景/破棄/失敗/旧世代で remove と cancel を呼ぶ。
// 守らない範囲: GMS 側の配信間隔と遅延、背景中の受信、過去の履歴。種別・確度はログに出さない。実機確認は #603。
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
    private static final String EXTRA_GEN = "com.fandhe.androidapp.activityrecognition.GEN";

    private static native void nativeOnBackgrounded();
    private static native void nativeOnResumed();

    private static final Object LOCK = new Object();
    // start 成功〜stop までの購読意思（Activity 再生成をまたぐ）。新インスタンスの read が LOST かの区別に使う。
    private static volatile boolean subscribed;
    // Rust から start が 1 度でも呼ばれたか。呼ばれる前は native を呼ばない（feature `device` 無効でも安全）。
    private static boolean nativeReady;       // LOCK の内側
    private static long gen;                  // LOCK の内側。購読のたびに +1
    private static long activeGen;            // LOCK の内側。0=購読なし
    private static long seq;                  // LOCK の内側。受理した配送ごとに +1、巻き戻さない
    private static long[] latest;             // LOCK の内側。(type, conf) の組。null=値なし
    private static long latestSeq;

    private final ComponentActivity activity;
    private final ActivityResultLauncher<String> permissionLauncher;
    private boolean wanted;     // LOCK の内側
    private boolean stopped;    // onStop〜onStart の間
    private boolean destroyed;
    private boolean awaiting;   // 許可ダイアログ待ち
    private long failure;       // 0 または ST_DENIED / ST_FAILED

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、MainActivity のフィールド初期化子で生成する。
    ActivityRecognitionHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.registerForActivityResult(
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
    }

    int start() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || !GmsAvailability.isAvailable(activity)) {
            return START_UNSUPPORTED;
        }
        synchronized (LOCK) {
            nativeReady = true;
            if (destroyed) return START_FAILED;
            subscribed = true;
            if (wanted) return START_OK;
            wanted = true;
            failure = 0;
            begin();
            return START_OK;
        }
    }

    long[] read() {
        synchronized (LOCK) {
            if (!wanted) return new long[] {(subscribed || destroyed) ? ST_LOST : ST_WAITING, 0};
            if (failure != 0) return new long[] {failure, 0};
            if (awaiting) return new long[] {ST_AWAITING, 0};
            if (stopped || latest == null) return new long[] {ST_WAITING, 0};
            long[] out = new long[2 + latest.length];
            out[0] = ST_READING;
            out[1] = latestSeq;
            System.arraycopy(latest, 0, out, 2, latest.length);
            return out;
        }
    }

    void stop() {
        synchronized (LOCK) {
            subscribed = false;
            wanted = false;
            failure = 0;
            unsubscribe();
        }
    }

    void onStart() {
        boolean notify;
        synchronized (LOCK) {
            stopped = false;
            if (wanted && !destroyed && failure == 0) begin();
            notify = nativeReady;
        }
        if (notify) nativeOnResumed();
    }

    void onStop() {
        boolean notify;
        synchronized (LOCK) {
            stopped = true;
            unsubscribe();
            notify = nativeReady;
        }
        if (notify) nativeOnBackgrounded();
    }

    void onDestroy() {
        synchronized (LOCK) {
            destroyed = true;
            wanted = false;
            unsubscribe();
        }
    }

    // LOCK の内側で呼ぶ。許可を確かめ、未許可ならダイアログ、許可済みなら購読する。背景中は onStart まで待つ。
    private void begin() {
        if (stopped || destroyed) return;
        if (activity.checkSelfPermission(Manifest.permission.ACTIVITY_RECOGNITION)
                != PackageManager.PERMISSION_GRANTED) {
            if (!awaiting) {
                awaiting = true;
                activity.runOnUiThread(() -> permissionLauncher.launch(Manifest.permission.ACTIVITY_RECOGNITION));
            }
            return;
        }
        subscribe();
    }

    // 許可ダイアログ自体が Activity を pause させるため、resumed では分岐しない。
    private void onPermissionResult(boolean granted) {
        synchronized (LOCK) {
            awaiting = false;
            if (!wanted || destroyed) return;
            if (!granted) {
                failure = ST_DENIED;
            } else if (!stopped) {
                subscribe(); // 背景中に届いた結果は購読せず onStart に任せる
            }
        }
    }

    // LOCK の内側で呼ぶ。新世代で購読する。
    private void subscribe() {
        unsubscribe();
        final long g = ++gen;
        activeGen = g;
        final Context app = activity.getApplicationContext();
        PendingIntent pi = pendingIntent(app, g);
        try {
            ActivityRecognition.getClient(app)
                    .requestActivityUpdates(DETECTION_INTERVAL_MS, pi)
                    .addOnSuccessListener(v -> onRequestSucceeded(app, g))
                    .addOnFailureListener(e -> onRequestFailed(app, g, e));
        } catch (SecurityException e) {
            fail(app, g, ST_DENIED);
        } catch (RuntimeException e) {
            fail(app, g, ST_FAILED);
        }
    }

    // LOCK の内側で呼ぶ。
    private void unsubscribe() {
        if (activeGen != 0) {
            removeUpdates(activity.getApplicationContext(), activeGen);
            activeGen = 0;
        }
        latest = null;
    }

    // LOCK の内側で呼ぶ。現世代の失敗だけを状態へ入れ、その購読を回収する。
    private void fail(Context app, long g, long code) {
        if (activeGen != g) return;
        activeGen = 0;
        failure = code;
        removeUpdates(app, g);
    }

    // 登録の完了が遅れて届いた場合、旧世代・停止済みならその世代だけを remove する。
    private void onRequestSucceeded(Context app, long g) {
        synchronized (LOCK) {
            if (activeGen != g || !wanted) removeUpdates(app, g);
        }
    }

    private void onRequestFailed(Context app, long g, Exception e) {
        synchronized (LOCK) {
            fail(app, g, e instanceof SecurityException ? ST_DENIED : ST_FAILED);
        }
    }

    // 配送先。世代が現在と違えば値を捨て、その世代の購読を回収する（孤児購読を含む）。
    static void onResult(Context context, Intent intent) {
        if (intent == null) return;
        Context app = context.getApplicationContext();
        long g = intent.getLongExtra(EXTRA_GEN, -1L);
        synchronized (LOCK) {
            if (g <= 0) return;
            if (g != activeGen) {
                removeUpdates(app, g);
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
            latest = pairs;
            latestSeq = ++seq;
        }
    }

    // 同じ requestCode の PendingIntent を作り直して購読を解除し、完了後に cancel する。
    private static void removeUpdates(Context app, long g) {
        final PendingIntent pi = pendingIntent(app, g);
        try {
            ActivityRecognition.getClient(app).removeActivityUpdates(pi)
                    .addOnCompleteListener(t -> pi.cancel());
        } catch (RuntimeException e) {
            pi.cancel();
        }
    }

    // GMS が extras を書き込むため API 31 以上は FLAG_MUTABLE。明示コンポーネント宛てなので他アプリには横取りされない。
    private static PendingIntent pendingIntent(Context app, long g) {
        Intent i = new Intent(app, ActivityRecognitionReceiver.class).putExtra(EXTRA_GEN, g);
        int flags = PendingIntent.FLAG_UPDATE_CURRENT
                | (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ? PendingIntent.FLAG_MUTABLE : 0);
        return PendingIntent.getBroadcast(app, (int) g, i, flags);
    }
}
