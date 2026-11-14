package com.fandhe.platform;

import android.app.Activity;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;

import java.util.Collections;
import java.util.Map;
import java.util.WeakHashMap;

// 傾きセンサ（Issue #831、Phase 10 型サンプル「傾けると玉が転がる」の Android 実センサ経路）の Android 実装ヘルパー。
// Activity ごとに 1 つ、`TiltMotionLifecycle`（`FeatureLifecycleProvider` 継承）が `attach` で生成して `HELPERS` に持つ。
// Rust の `fandhe-cap-tilt-motion`（`src/android.rs`）が、画面が傾きを使うかの判定（`set_screen_active`）と、
// 重力センサ値の受け口（`nativeOnMotionChanged`）を持つ。Rust から Java へ呼ぶ入口は無い（値は Java から Rust へ push するだけ）。
//
// 取得元は重力センサ（TYPE_GRAVITY）で、無ければ加速度センサ（TYPE_ACCELEROMETER）へ落とす。加速度センサは
// ユーザー操作由来の線形加速度成分を含むため傾き検出にノイズが乗り得るが、実装は追加しない（気になる場合は
// ローパスフィルタを足す、#831 実装計画 §8）。`registerListener` / `unregisterListener` は onResume / onPause で対にする
// （Android の推奨作法。バックグラウンド中の無駄なセンサ計測・バッテリ消費を避ける）。
//
// x, y: 重力センサの生値（SensorEvent#values[0]/[1]、m/s^2）。rotation: 呼び出し時点の画面回転（Surface.ROTATION_*）。
// Rust 側 `gravity_to_tilt` が画面座標の傾きへ写像する。値は毎回 Rust へ渡し、画面が傾きを使わないときの再描画の抑制は
// Rust 側（`set_screen_active`）が担う。センサ値はログに出さない。
final class TiltMotionHelper {
    // `x`, `y`: 重力センサ（TYPE_GRAVITY、無ければ TYPE_ACCELEROMETER）の生値。`rotation`: `Surface.ROTATION_*`。
    private static native void nativeOnMotionChanged(float x, float y, int rotation);

    // Activity ごとの helper（上記クラス冒頭コメント参照）。
    private static final Map<Activity, TiltMotionHelper> HELPERS =
            Collections.synchronizedMap(new WeakHashMap<>());

    private final Activity activity;
    private final SensorManager sensorManager;
    private final Sensor motionSensor;

    private final SensorEventListener motionListener = new SensorEventListener() {
        @Override
        public void onSensorChanged(SensorEvent event) {
            // 呼び出しごとに取得する（ponytail: 回転変化のたびに読み直すだけの軽量呼び出し。負荷が問題になったら
            // onConfigurationChanged でキャッシュ化、#831 実装計画 §5 ステップ 3）。
            @SuppressWarnings("deprecation") // minSdk 24 のため Activity#getDisplay()（API 30+）は使えない
            int rotation = activity.getWindowManager().getDefaultDisplay().getRotation();
            nativeOnMotionChanged(event.values[0], event.values[1], rotation);
        }

        @Override
        public void onAccuracyChanged(Sensor sensor, int accuracy) {
            // 精度変化の通知は使わない。
        }
    };

    private TiltMotionHelper(Activity activity) {
        this.activity = activity;
        this.sensorManager = activity.getSystemService(SensorManager.class);
        Sensor sensor = null;
        if (sensorManager != null) {
            sensor = sensorManager.getDefaultSensor(Sensor.TYPE_GRAVITY);
            if (sensor == null) {
                sensor = sensorManager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER);
            }
        }
        this.motionSensor = sensor;
    }

    // Activity の生成時（`TiltMotionLifecycle` から、UI スレッド）。
    static void attach(Activity activity) {
        HELPERS.put(activity, new TiltMotionHelper(activity));
    }

    // Activity の破棄時（同上）。再生成を含む。リスナーを解除する。
    static void detach(Activity activity) {
        TiltMotionHelper helper = HELPERS.remove(activity);
        if (helper != null) {
            helper.unregister();
        }
    }

    // `TiltMotionLifecycle` から（UI スレッド）。前面へ戻るたびに登録する（`onPause` で解除済みのため）。
    // `Activity.onResume()` の中、つまり `super.onResume()` の中で届く。従来の `MainActivity` は `super` の後で
    // 登録していたため、`super` に対する前後だけが違う。
    static void resumed(Activity activity) {
        TiltMotionHelper helper = HELPERS.get(activity);
        if (helper != null && helper.sensorManager != null && helper.motionSensor != null) {
            helper.sensorManager.registerListener(
                    helper.motionListener, helper.motionSensor, SensorManager.SENSOR_DELAY_GAME);
        }
    }

    static void paused(Activity activity) {
        TiltMotionHelper helper = HELPERS.get(activity);
        if (helper != null) {
            helper.unregister();
        }
    }

    private void unregister() {
        if (sensorManager != null) {
            sensorManager.unregisterListener(motionListener);
        }
    }
}
