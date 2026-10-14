package com.fandhe.androidapp;

import android.content.Context;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.os.SystemClock;

// 方位（コンパス）画面（Issue #1406、親 #1154）の Android 実装ヘルパー。MainActivity から
// 生成・委譲される（Rust の `device/android_compass.rs` が egui スレッドから
// `MainActivity.compassStart` / `compassRead` / `compassStop` を同期 JNI で呼ぶ）。
//
// 戻り値のコード・ビット配置の SSOT はここで、Rust 側 `android_compass.rs` が同じ値で解釈する。
//   compassStart: 0=OK / 1=センサー無し / 2=登録失敗・破棄済み
//   compassRead : 負値=購読が失われた / 0=値なし / bit32=1 の値あり
//                 （bit0-31=方位角ラジアンの float 生ビット、bit40-47=event.accuracy & 0xff）
//
// スレッド: start/read/stop は egui スレッド、onSensorChanged は main looper。登録・解除は
// 任意スレッドから可能なため、すべて `lock` の内側で直接行う（main への同期待ちを作らない）。
// 磁北のみ。偏角補正・画面回転の補正はしない。方位・精度はログに出さない。
//
// ライフサイクル: `onPause` は背景計測を避けて解除するが購読の意思は残す（read は値なし）。
// `onResume` で新しいリスナーで再登録。`onDestroy`（再生成を含む）は購読を破棄し、
// 新インスタンスは未購読から始まるため read が負値を返し Rust 側が失敗として扱う。
final class CompassHelper {
    static final int START_OK = 0;
    static final int START_UNSUPPORTED = 1;
    static final int START_FAILED = 2;
    static final long READ_LOST = -1L;
    // ponytail: 古い値の閾値。センサーが止まっても最大 1 秒は最後の値を返す。短縮は要調整。
    private static final long STALE_MS = 1000L;

    // start 成功〜stop までの購読意思（プロセス内で Activity 再生成をまたぐ）。新インスタンスの
    // read が「未開始（値なし）」か「破棄で購読喪失（READ_LOST）」かの区別に使う。
    private static volatile boolean subscribed;

    private final Context context;
    // Activity は onCreate 前に getSystemService できないため、start 初回に遅延取得する。
    private SensorManager manager; // lock の内側
    private final Object lock = new Object();
    private boolean wanted;     // 購読の意思（lock の内側）
    private boolean paused;
    private boolean foreground = true; // Activity の前景状態（購読意思と独立）
    private boolean resumeFailed;      // onResume 時の再登録失敗（read が失敗を返す）
    private boolean destroyed;
    private Listener current;   // 現世代のリスナー。null は未登録
    private long packed;        // 最新値（0=なし）
    private long receivedAt;    // elapsedRealtime（単調時計）

    CompassHelper(Context context) {
        this.context = context;
    }

    int start() {
        synchronized (lock) {
            if (destroyed) return START_FAILED;
            if (manager == null) {
                manager = (SensorManager) context.getSystemService(Context.SENSOR_SERVICE);
            }
            if (manager == null) return START_UNSUPPORTED;
            if (wanted) {
                subscribed = true;
                return START_OK;
            }
            if (manager.getDefaultSensor(Sensor.TYPE_ROTATION_VECTOR) == null
                    && (manager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER) == null
                    || manager.getDefaultSensor(Sensor.TYPE_MAGNETIC_FIELD) == null)) {
                return START_UNSUPPORTED;
            }
            wanted = true;
            resumeFailed = false;
            if (!foreground) {
                // 背景中の start は登録を onResume まで延期する。
                paused = true;
                subscribed = true;
                return START_OK;
            }
            if (!register()) {
                wanted = false;
                return START_FAILED;
            }
            paused = false;
            subscribed = true;
            return START_OK;
        }
    }

    long read() {
        synchronized (lock) {
            // 未開始・明示 stop 後は値なし。破棄・再生成で購読が失われた場合のみ負値。
            if (!wanted) return (subscribed || destroyed) ? READ_LOST : 0L;
            if (resumeFailed) return READ_LOST;
            if (paused || packed == 0L) return 0L;
            if (SystemClock.elapsedRealtime() - receivedAt > STALE_MS) return 0L;
            return packed;
        }
    }

    void stop() {
        synchronized (lock) {
            subscribed = false;
            wanted = false;
            resumeFailed = false;
            unregister();
        }
    }

    void onPause() {
        synchronized (lock) {
            foreground = false;
            if (wanted && !paused) {
                paused = true;
                unregister();
            }
        }
    }

    void onResume() {
        synchronized (lock) {
            foreground = true;
            if (wanted && paused && !destroyed) {
                // 失敗は resumeFailed として保持し、read が購読喪失（負値）を返す。
                if (register()) paused = false;
                else resumeFailed = true;
            }
        }
    }

    void onDestroy() {
        synchronized (lock) {
            destroyed = true;
            wanted = false;
            unregister();
        }
    }

    // lock の内側で呼ぶ。新世代のリスナーで登録し、途中失敗なら登録済み分も解除する。
    private boolean register() {
        unregister();
        Listener l = new Listener();
        Sensor rv = manager.getDefaultSensor(Sensor.TYPE_ROTATION_VECTOR);
        boolean ok;
        if (rv != null) {
            ok = manager.registerListener(l, rv, SensorManager.SENSOR_DELAY_UI);
        } else {
            ok = manager.registerListener(l,
                    manager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER), SensorManager.SENSOR_DELAY_UI)
                    && manager.registerListener(l,
                    manager.getDefaultSensor(Sensor.TYPE_MAGNETIC_FIELD), SensorManager.SENSOR_DELAY_UI);
        }
        if (!ok) {
            manager.unregisterListener(l);
            return false;
        }
        current = l;
        return true;
    }

    private void unregister() {
        if (current != null && manager != null) {
            manager.unregisterListener(current);
        }
        current = null;
        packed = 0L;
    }

    private final class Listener implements SensorEventListener {
        private final float[] accel = new float[3];
        private final float[] mag = new float[3];
        private long accelAt;   // 0=未受信。センサーごとの受信時刻（単調時計）
        private long magAt;
        private int accelAcc;
        private int magAcc;
        private int accuracy;
        private final float[] matrix = new float[9];
        private final float[] orientation = new float[3];

        @Override
        public void onSensorChanged(SensorEvent event) {
            synchronized (lock) {
                if (current != this) return; // 解除後・旧世代の値は捨てる
                switch (event.sensor.getType()) {
                    case Sensor.TYPE_ROTATION_VECTOR:
                        SensorManager.getRotationMatrixFromVector(matrix, event.values);
                        accuracy = event.accuracy;
                        receivedAt = SystemClock.elapsedRealtime();
                        break;
                    case Sensor.TYPE_ACCELEROMETER:
                        System.arraycopy(event.values, 0, accel, 0, 3);
                        accelAt = SystemClock.elapsedRealtime();
                        accelAcc = event.accuracy;
                        if (magAt == 0L
                                || !rotationOk()) return;
                        break;
                    case Sensor.TYPE_MAGNETIC_FIELD:
                        System.arraycopy(event.values, 0, mag, 0, 3);
                        magAt = SystemClock.elapsedRealtime();
                        magAcc = event.accuracy;
                        if (accelAt == 0L
                                || !rotationOk()) return;
                        break;
                    default:
                        return;
                }
                SensorManager.getOrientation(matrix, orientation);
                if (event.sensor.getType() != Sensor.TYPE_ROTATION_VECTOR) {
                    // 片方が止まれば古い側の時刻が残り read の鮮度判定で値なしになる。
                    // 精度は両センサーの低い方。どちらかが NO_CONTACT 等（負）なら Unknown(255)。
                    receivedAt = Math.min(accelAt, magAt);
                    accuracy = (accelAcc < 0 || magAcc < 0) ? -1 : Math.min(accelAcc, magAcc);
                }
                long bits = Float.floatToRawIntBits(orientation[0]) & 0xffffffffL;
                packed = bits | (1L << 32) | ((long) (accuracy & 0xff) << 40);
            }
        }

        // 合成失敗（加速度 0 等）なら直前の方位で代用せず値なしへ戻す。
        private boolean rotationOk() {
            if (SensorManager.getRotationMatrix(matrix, null, accel, mag)) return true;
            packed = 0L;
            return false;
        }

        @Override
        public void onAccuracyChanged(Sensor sensor, int accuracy) {}
    }
}
