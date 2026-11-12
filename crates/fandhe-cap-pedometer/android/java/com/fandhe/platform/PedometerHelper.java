package com.fandhe.platform;

import android.Manifest;
import android.app.Activity;
import android.content.Context;
import android.content.pm.PackageManager;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.os.Build;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// 歩数画面（Issue #1249、#1187）の Android 実装ヘルパー。利用者の `MainActivity` には何も書かせない。
// Rust の `src/android.rs` が static の `read(Activity, id)` / `cancel(Activity, id)` を JNI で呼び、
// 結果を `nativeOnPedometerResult` で受ける。Activity への参加は `PedometerLifecycle`
// （非公開 ContentProvider、`FeatureLifecycleProvider` 継承）が行い、生成時に `attach`、破棄時に `detach` を呼ぶ。
// Activity ごとの helper は `HELPERS` に持つ（Rust のワーカーと UI スレッドの両方から触るので同期化し、
// 破棄時に必ず外す）。表に無い Activity への要求は即座に INTERRUPTED で返す。
//
// 1 回の要求 = `TYPE_STEP_COUNTER` のリスナーを登録し、最初のイベント（再起動後の
// 累積歩数）を 1 回受けて解除する。開始・許可結果は UI スレッド（main looper）で直列化する。
// `cancel` は UI キューの停滞に依存しないよう呼び出しスレッドで即時に解除するため、
// リスナーの登録・解除と要求 id の照合だけは `lock` の内側で行う（登録直前に id を
// 再確認するので、cancel 後に遅れて開始した要求がリスナーを残すことはない）。
//
// ライフサイクル: `onPause` では破棄しない（リスナーは Rust 側の期限 15 秒後の
// `cancel` で必ず解除されるため、背景での残存に上限がある）。`onDestroy`（Activity 再生成を
// 含む）では保留中の要求を INTERRUPTED で確定して破棄する。`request` はインスタンス
// フィールドなので、再生成後の新インスタンスは NONE から始まり旧結果を拾わない。
final class PedometerHelper implements SensorEventListener {
    // 結果コード。SSOT はここで、Rust 側 `android.rs::decode` が同じ値で写像する。
    static final int RESULT_OK = 0;
    static final int RESULT_UNSUPPORTED = 1;
    static final int RESULT_PERMISSION_DENIED = 2;
    static final int RESULT_INTERRUPTED = 3;
    static final int RESULT_FAILED = 4;

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, PedometerHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private static final String KEY_PERMISSION = "fandhe.pedometer.permission";

    private static native void nativeOnPedometerResult(long requestId, int code, long steps);

    private final ComponentActivity activity;
    private final RequestTracker request = new RequestTracker();
    private SensorManager sensorManager;
    private final Object lock = new Object();
    private boolean listening; // lock の内側でのみ読み書きする。
    // 取り消し済みの最大 id（lock の内側）。cancel が start より先に走っても古い要求を開始しない。
    private long cancelledId = RequestTracker.NONE;
    // 許可ダイアログを起動した要求 id（UI スレッド専用）。NONE ならダイアログは出ていない。
    private long permissionId = RequestTracker.NONE;
    // UI スレッドで最後に受理した id。id は単調増加のため、これ以下の開始要求は
    // 追い越された古い要求として捨てる（Rust 側は既に NoReading で戻っている）。
    private long latestId = RequestTracker.NONE;

    // `registerForActivityResult` は STARTED より前に呼ぶ契約のため、`attach`（`Activity.onCreate` 内）で
    // 本クラスを生成してここで登録する。キーは明示する（連番だと生成順で結果が別のランチャーへ届く）。
    private final ActivityResultLauncher<String> permissionLauncher;

    private PedometerHelper(ComponentActivity activity) {
        this.activity = activity;
        this.permissionLauncher = activity.getActivityResultRegistry().register(KEY_PERMISSION, activity,
                new ActivityResultContracts.RequestPermission(), this::onPermissionResult);
    }

    // Activity の生成時（`PedometerLifecycle` から）。
    static void attach(ComponentActivity activity) {
        HELPERS.put(activity, new PedometerHelper(activity));
    }

    // Activity の破棄時（`PedometerLifecycle` から）。保留中の要求を中断で確定し、表から外す。
    static void detach(Activity activity) {
        PedometerHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.onDestroy();
        }
    }

    // JNI スレッド（Rust のワーカー）から呼ばれる入口。helper が無い（生成前・破棄後）なら
    // Rust を期限まで待たせず、中断として即座に返す。
    static void read(Activity activity, long id) {
        PedometerHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.begin(id);
        } else {
            nativeOnPedometerResult(id, RESULT_INTERRUPTED, 0);
        }
    }

    // Rust 側の期限切れから呼ばれる入口。
    static void cancel(Activity activity, long id) {
        PedometerHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.cancelRequest(id);
        }
    }

    private void begin(long id) {
        activity.runOnUiThread(() -> start(id));
    }

    // Rust 側の期限切れから呼ばれる。古い id なら何もしない。戻る前にリスナーを解除する
    // （UI スレッドへ post しない: UI が停滞しても期限後にリスナーが残らないようにする）。
    private void cancelRequest(long id) {
        synchronized (lock) {
            cancelledId = Math.max(cancelledId, id);
            if (request.complete(id)) {
                unregister();
            }
        }
    }

    // `detach` から UI スレッドで呼ぶ。
    private void onDestroy() {
        finish(RESULT_INTERRUPTED, 0);
    }

    private boolean destroyed() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    private void start(long id) {
        if (id <= latestId) {
            return;
        }
        latestId = id;
        if (destroyed()) {
            nativeOnPedometerResult(id, RESULT_INTERRUPTED, 0);
            return;
        }
        synchronized (lock) {
            if (id <= cancelledId) {
                return;
            }
            unregister();
            request.track(id);
        }
        if (sensorManager == null) {
            sensorManager = (SensorManager) activity.getSystemService(Context.SENSOR_SERVICE);
        }
        if (sensorManager == null || sensorManager.getDefaultSensor(Sensor.TYPE_STEP_COUNTER) == null) {
            finish(RESULT_UNSUPPORTED, 0);
            return;
        }
        checkPermissionAndListen(id);
    }

    private void checkPermissionAndListen(long id) {
        // ACTIVITY_RECOGNITION は API 29 以上でのみ実行時許可の対象。
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q
                && activity.checkSelfPermission(Manifest.permission.ACTIVITY_RECOGNITION)
                        != PackageManager.PERMISSION_GRANTED) {
            // ダイアログは同時に 1 つだけ。出ている間に差し替わった要求は結果到着時に再判定する。
            if (permissionId == RequestTracker.NONE) {
                permissionId = id;
                permissionLauncher.launch(Manifest.permission.ACTIVITY_RECOGNITION);
            }
            return;
        }
        startListening(id);
    }

    // 許可ダイアログ自体が Activity を pause させるため、resumed では分岐しない。
    private void onPermissionResult(boolean granted) {
        long launched = permissionId;
        permissionId = RequestTracker.NONE;
        long current = request.current();
        if (current == RequestTracker.NONE) {
            return; // 期限切れ・破棄済み・再生成後のインスタンス。
        }
        if (destroyed()) {
            finish(RESULT_INTERRUPTED, 0);
        } else if (launched != current) {
            // 起動時の要求は差し替え済み。古い結果は捨て、現在の要求で許可を再判定する。
            checkPermissionAndListen(current);
        } else if (!granted) {
            finish(RESULT_PERMISSION_DENIED, 0);
        } else {
            startListening(current);
        }
    }

    private void startListening(long id) {
        Sensor sensor = sensorManager.getDefaultSensor(Sensor.TYPE_STEP_COUNTER);
        boolean ok;
        synchronized (lock) {
            if (request.current() != id) {
                return; // 取り消し・差し替え済み。登録しない。
            }
            ok = sensor != null
                    && sensorManager.registerListener(this, sensor, SensorManager.SENSOR_DELAY_NORMAL);
            listening = ok;
        }
        if (!ok) {
            finish(RESULT_FAILED, 0);
        }
    }

    // 任意のスレッドから呼べる（lock で直列化）。
    private void unregister() {
        synchronized (lock) {
            unregisterLocked();
        }
    }

    private void unregisterLocked() {
        if (listening && sensorManager != null) {
            sensorManager.unregisterListener(this);
        }
        listening = false;
    }

    // 保留中の要求を 1 回だけ確定してリスナーを解除する。取り出しと解除は cancel と同じ lock 内で
    // 不可分に行い（期限切れが間に入ってもリスナーが残らない）、native 通知は lock の外で行う。
    private void finish(int code, long steps) {
        long id;
        synchronized (lock) {
            id = request.take();
            unregisterLocked();
        }
        if (id != RequestTracker.NONE) {
            nativeOnPedometerResult(id, code, steps);
        }
    }

    @Override
    public void onSensorChanged(SensorEvent event) {
        if (request.current() == RequestTracker.NONE) {
            unregister();
            return;
        }
        float v = event.values.length > 0 ? event.values[0] : Float.NaN;
        if (Float.isNaN(v) || Float.isInfinite(v) || v < 0f) {
            finish(RESULT_FAILED, 0);
        } else {
            finish(RESULT_OK, (long) v);
        }
    }

    @Override
    public void onAccuracyChanged(Sensor sensor, int accuracy) {}
}
